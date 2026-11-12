//! What the repositories panel does to repositories: branch, commit, push,
//! fetch, pull, merge or rebase onto the default branch, and see a
//! conflicted operation through or abort it; and what it
//! checks before (whether a merge would conflict, what blocks a switch).
//!
//! Each runs `git` off the event loop, never waiting on a prompt: no
//! terminal for passwords (`GIT_TERMINAL_PROMPT=0`, ssh in batch mode), no
//! editor (`GIT_EDITOR=true`), English messages (`LC_ALL=C`) so that what
//! went wrong can be told from them, and a time limit. Each returns every
//! command it ran with what it printed, for the panel's log, and on
//! failure what kind it was, for the panel to offer what fixes it.

use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crate::{
    desktop::shell_quote,
    diff::{self, FileDiff},
    git::{self, Change, InProgress},
    process,
};

/// How long a command that talks to a remote may take before it is killed.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a local one may take (a commit hook, a signing prompt).
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait between looks at whether a command has finished, at most.
const POLL: Duration = Duration::from_millis(20);
/// How long a command stopped by the user has to end before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(1);
/// How long a command's output may still take to arrive once it has ended:
/// something it left running (a detached `gc`) may hold the pipe open.
const GRACE: Duration = Duration::from_secs(2);
/// How many lines of a repository's diff are read.
const REPO_DIFF_LINES: usize = 20_000;
/// How many of an untracked folder's files are listed.
const FOLDER_FILES: usize = 200;
/// An untracked file bigger than this isn't shown.
const UNTRACKED_BYTES: u64 = 1 << 20;
/// How many commits `log_upstream` shows when there are none to push or
/// pull.
const LOG_LAST: &str = "10";
/// How many commits to push, and to pull, `log_upstream` lists at most.
const LOG_MAX: usize = 200;
/// How long to wait before each new try of a command that found a lock
/// held: another git (the user's, an editor's) is likely done by then.
#[cfg(not(test))]
const LOCK_RETRIES: [Duration; 3] = [
    Duration::from_millis(300),
    Duration::from_secs(1),
    Duration::from_secs(2),
];
#[cfg(test)]
const LOCK_RETRIES: [Duration; 3] = [Duration::from_millis(10); 3];

/// A command that ran, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    /// The command as a shell line, `git push -u origin HEAD`, quoted where
    /// needed: what "Run in terminal" runs again, in the repository's
    /// folder, for the user to answer its prompts.
    pub command: String,
    /// What it printed, stdout then stderr.
    pub output: String,
    pub ok: bool,
}

/// Why an operation stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// It stopped on conflicts in these files (from the worktree's top
    /// folder); the repository is left mid-operation to resolve.
    Conflict(Vec<String>),
    /// A password, passphrase or login was wanted.
    Auth,
    /// The remote has commits the branch doesn't: push refused.
    Rejected,
    /// The branch and its upstream both have commits: no fast-forward.
    Diverged,
    /// The branch is checked out in the worktree at this path.
    CheckedOut(PathBuf),
    /// Another git holds a lock: `index.lock`, a ref's, `packed-refs`'.
    Locked,
    /// git doesn't know who commits: no `user.name` or `user.email` set,
    /// and none it could guess.
    Identity,
    /// It took longer than allowed and was killed.
    Timeout,
    /// It wasn't tried: why (`"detached HEAD"`, `"no remote"`, `"HEAD
    /// moved since"`).
    Blocked(String),
    /// Anything else: a hook said no, a bad name. The log says what.
    Other,
}

/// An operation that stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    pub failure: Failure,
    /// One line on why, for the row and a toast: git's last line, with
    /// `fatal: ` / `error: ` stripped.
    pub why: String,
    /// Every command run, the failed one last.
    pub log: Vec<Ran>,
}

/// What an operation did: the commands it ran, or why it stopped.
pub type OpResult = Result<Vec<Ran>, Failed>;

/// Where a checkout stands, recorded before a batch so it can be undone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Head {
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub oid: Option<String>,
}

/// How a pull joins the upstream's commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    /// Only when nothing is lost: else `Failure::Diverged`.
    FastForward,
    Rebase,
    Merge,
}

/// How the default branch's new commits come into the checked-out branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrate {
    Merge,
    Rebase,
}

/// What the check before an operation found for a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    /// These files would conflict (for a rebase: may).
    Conflicts(Vec<String>),
    /// It can't be done here: why.
    Blocked(String),
    /// Deleting the branch loses this many commits no other branch, remote
    /// branch or tag has.
    Unmerged(usize),
}

/// Whether a command talks to a remote: it gets longer, and ssh in batch
/// mode. A `Read` only asks: it takes no lock it can do without
/// (`GIT_OPTIONAL_LOCKS=0`), so it never gets in an operation's way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Read,
    Local,
    Network,
}

/// What a command printed and how it ended.
struct Output {
    ran: Ran,
    /// Its exit code; `None` when it was killed or didn't start.
    code: Option<i32>,
    stdout: String,
    stderr: String,
    /// It was killed after this long.
    timed_out: Option<Duration>,
    /// It was stopped, asked to by the user (`Watch::stop`).
    stopped: bool,
}

/// What a thread running commands for a batch is watched by: a flag the
/// user sets to stop it (its commands killed, those after not run), and
/// where to say what it waits for (a lock held, to try again).
pub struct Watch {
    pub stop: Arc<AtomicBool>,
    pub say: Box<dyn Fn(&str)>,
}

thread_local! {
    /// The watch over the commands this thread runs, if any.
    static WATCH: std::cell::RefCell<Option<Watch>> = const { std::cell::RefCell::new(None) };
}

/// Watch the commands this thread runs from now on with `watch`; `None`
/// to stop.
pub fn watch(watch: Option<Watch>) {
    WATCH.with(|w| *w.borrow_mut() = watch);
}

/// Whether this thread's commands were asked to stop.
fn stopping() -> bool {
    WATCH.with(|w| {
        w.borrow()
            .as_ref()
            .is_some_and(|w| w.stop.load(AtomicOrdering::Relaxed))
    })
}

/// Say `what` this thread waits for, to its watch.
fn say(what: &str) {
    WATCH.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            (w.say)(what);
        }
    });
}

/// Why a step wasn't done: the user stopped the batch.
pub const STOPPED: &str = "stopped";

#[cfg(test)]
thread_local! {
    /// A shorter time limit for the commands this test thread runs, so a
    /// test can see a kill without waiting a minute.
    static TIMEOUT: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

/// How long a command may take.
fn limit(reach: Reach) -> Duration {
    #[cfg(test)]
    if let Some(limit) = TIMEOUT.with(|t| t.get()) {
        return limit;
    }
    match reach {
        Reach::Read | Reach::Local => LOCAL_TIMEOUT,
        Reach::Network => NETWORK_TIMEOUT,
    }
}

/// `git args` as a shell line, without the `-C dir` it runs with.
fn line(args: &[&str]) -> String {
    let mut line = String::from("git");
    for arg in args {
        line.push(' ');
        line.push_str(&shell_quote(arg));
    }
    line
}

/// The ssh command that keeps ssh from asking for a passphrase or a host
/// key, for a command that may reach a remote over ssh; `None` when the
/// user's own is left as it is.
fn batch_ssh(dir: &Path) -> Option<String> {
    // A GIT_SSH_COMMAND or GIT_SSH of the user's wins over ours; tests
    // never see the user's.
    let inherited = |var| cfg!(not(test)) && std::env::var_os(var).is_some();
    if inherited("GIT_SSH_COMMAND") || inherited("GIT_SSH") {
        return None;
    }
    // GIT_SSH_COMMAND would also win over core.sshCommand (a key of its
    // own, say), so that one gets batch mode added instead, when it is ssh.
    let own = query(dir, &["config", "core.sshCommand"]).unwrap_or_default();
    let own = own.trim();
    if own.is_empty() {
        return Some("ssh -o BatchMode=yes".into());
    }
    let program = own.split_whitespace().next().unwrap_or("");
    (Path::new(program).file_name()? == "ssh").then(|| format!("{own} -o BatchMode=yes"))
}

/// Run `git -C dir args`: never prompting, its output read as it comes,
/// killed (and what it started) when it takes too long.
fn exec(dir: &Path, args: &[&str], reach: Reach) -> Output {
    let mut cmd = git::command(dir, args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_MERGE_AUTOEDIT", "no")
        // Its own group, so that a kill takes its hooks and ssh too.
        .process_group(0);
    if reach == Reach::Network
        && let Some(ssh) = batch_ssh(dir)
    {
        cmd.env("GIT_SSH_COMMAND", ssh);
    }
    // Only a read leaves index.lock alone; the rest commit, stash, merge.
    if reach != Reach::Read {
        cmd.env_remove("GIT_OPTIONAL_LOCKS");
    }
    #[cfg(test)]
    cmd.env_remove("GIT_SSH");
    let finish = |stdout: String,
                  stderr: String,
                  code: Option<i32>,
                  timed_out: Option<Duration>,
                  stopped: bool| {
        let mut output = stdout.clone();
        if !output.is_empty() && !output.ends_with('\n') && !stderr.is_empty() {
            output.push('\n');
        }
        output.push_str(&stderr);
        if let Some(after) = timed_out {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("(killed after {} s)\n", after.as_secs_f32()));
        }
        Output {
            ran: Ran {
                command: line(args),
                output,
                ok: code == Some(0),
            },
            code,
            stdout,
            stderr,
            timed_out,
            stopped,
        }
    };
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return finish(String::new(), format!("git: {err}\n"), None, None, false),
    };
    let stdout = Drain::new(child.stdout.take());
    let stderr = Drain::new(child.stderr.take());
    let timeout = limit(reach);
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    let kill = |child: &mut std::process::Child| {
        process::kill_group(process::group_of(child), libc::SIGKILL);
        let _ = child.wait();
    };
    // Stopped by the user: asked to end first, so that git lets go of its
    // locks, then killed after a while.
    let stop = |child: &mut std::process::Child| process::stop_group(child, STOP_GRACE);
    let (code, timed_out, stopped) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), None, false),
            Ok(None) if stopping() => {
                stop(&mut child);
                break (None, None, true);
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(POLL);
            }
            Ok(None) => {
                kill(&mut child);
                break (None, Some(timeout), false);
            }
            Err(_) => {
                kill(&mut child);
                break (None, None, false);
            }
        }
    };
    finish(stdout.finish(), stderr.finish(), code, timed_out, stopped)
}

/// A pipe read to its end on a thread of its own, so that a command never
/// stalls on a full one while its other one is waited on.
struct Drain {
    read: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl Drain {
    fn new(pipe: Option<impl Read + Send + 'static>) -> Self {
        let read = Arc::<Mutex<Vec<u8>>>::default();
        let (tx, done) = mpsc::channel();
        if let Some(mut pipe) = pipe {
            let read = read.clone();
            let _ = std::thread::Builder::new()
                .name("git output".into())
                .spawn(move || {
                    let mut chunk = [0; 8192];
                    loop {
                        match pipe.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => read.lock().unwrap().extend_from_slice(&chunk[..n]),
                            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                    }
                    let _ = tx.send(());
                });
        }
        Self { read, done }
    }

    /// What was read, once the pipe is closed or after `GRACE`.
    fn finish(self) -> String {
        let _ = self.done.recv_timeout(GRACE);
        String::from_utf8_lossy(&self.read.lock().unwrap()).into_owned()
    }
}

/// What `git args` prints in `dir` when it succeeds: a question asked, not
/// logged.
fn query(dir: &Path, args: &[&str]) -> Option<String> {
    let out = exec(dir, args, Reach::Read);
    out.ran.ok.then_some(out.stdout)
}

/// The repositories gerdr runs a git command that writes in, by shared
/// git folder, each with what runs there: one at a time in each, or two
/// fetches (a background one and the user's push) fight over its refs'
/// locks and one fails.
static TURNS: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
/// Told when a turn is over.
static TURN_OVER: Condvar = Condvar::new();

/// What runs in each repository now, by shared git folder: `"a fetch"`.
pub fn turns() -> Vec<(PathBuf, String)> {
    TURNS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A repository's turn to be written in, by one thread, until dropped.
/// Taken around every operation the panel runs, in the background too.
#[must_use]
pub struct Turn(PathBuf);

impl Turn {
    /// The turn of the repository whose shared git folder is `key` (its
    /// folder when it has none), for `what` (`"a fetch"`); else what has
    /// it now.
    pub fn try_take(key: &Path, what: &str) -> Result<Self, String> {
        let mut turns = TURNS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, holder)) = turns.iter().find(|(k, _)| k == key) {
            return Err(holder.clone());
        }
        turns.push((key.to_owned(), what.to_owned()));
        Ok(Self(key.to_owned()))
    }

    /// The turn of `key` for `what`, waiting for it while this thread's
    /// commands aren't stopped (`Watch`); `None` once they are.
    pub fn take_unless_stopped(key: &Path, what: &str) -> Option<Self> {
        let mut turns = TURNS.lock().unwrap_or_else(|e| e.into_inner());
        while turns.iter().any(|(k, _)| k == key) {
            if stopping() {
                return None;
            }
            let wait = TURN_OVER.wait_timeout(turns, POLL * 5);
            turns = wait.unwrap_or_else(|e| e.into_inner()).0;
        }
        turns.push((key.to_owned(), what.to_owned()));
        Some(Self(key.to_owned()))
    }

    /// The turn of `key` for `what`, waiting for it.
    pub fn take(key: &Path, what: &str) -> Self {
        let mut turns = TURNS.lock().unwrap_or_else(|e| e.into_inner());
        while turns.iter().any(|(k, _)| k == key) {
            turns = TURN_OVER.wait(turns).unwrap_or_else(|e| e.into_inner());
        }
        turns.push((key.to_owned(), what.to_owned()));
        Self(key.to_owned())
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut turns = TURNS.lock().unwrap_or_else(|e| e.into_inner());
        turns.retain(|(k, _)| *k != self.0);
        TURN_OVER.notify_all();
    }
}

/// Add the commands of `log` that failed, run in `dir`, to gerdr's
/// `git.log`, with all they printed: what the panel showed is gone once
/// the next batch runs, and a background fetch's failure isn't shown.
#[cfg_attr(test, allow(unused_variables))]
fn log_failed(dir: &Path, log: &[Ran]) {
    #[cfg(not(test))]
    {
        use std::io::Write;
        let failed: Vec<&Ran> = log.iter().filter(|r| !r.ok).collect();
        if failed.is_empty() {
            return;
        }
        let Some(mut file) = crate::launch::append_log(&crate::state_dir().join("git.log")) else {
            return;
        };
        let mut text = String::new();
        for ran in failed {
            text.push_str(&format!(
                "{} {}\n$ {}\n{}",
                local_time(),
                dir.display(),
                ran.command,
                ran.output
            ));
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push('\n');
        }
        let _ = file.write_all(text.as_bytes());
    }
}

/// The local time now, `2026-10-27 14:03:52`.
#[cfg(not(test))]
fn local_time() -> String {
    // SAFETY: `localtime_r` writes only to the `tm` it is given, and an
    // all-zero `tm` is a valid one.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return String::new();
        }
        tm
    };
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// What kind of failure git's output tells of. A conflict's files are
/// left for the caller to ask git for.
pub fn classify(stderr: &str, stdout: &str) -> Failure {
    let text = format!("{stderr}\n{stdout}");
    let has = |s: &str| text.contains(s);
    if let Some(path) = checked_out_at(&text) {
        return Failure::CheckedOut(path);
    }
    // A ref named like a folder another has is no lock gone free later.
    let locked = has("index.lock")
        || has(".lock': File exists")
        || has("cannot lock ref") && !has("exists; cannot create")
        || has("could not lock config file");
    if locked {
        return Failure::Locked;
    }
    const IDENTITY: [&str; 4] = [
        "Author identity unknown",
        "Committer identity unknown",
        "unable to auto-detect email address",
        "empty ident name",
    ];
    if IDENTITY.iter().any(|s| has(s)) {
        return Failure::Identity;
    }
    const AUTH: [&str; 6] = [
        "Permission denied (publickey",
        "could not read Username",
        "could not read Password",
        "terminal prompts disabled",
        "Authentication failed",
        "Host key verification failed",
    ];
    if AUTH.iter().any(|s| has(s)) {
        return Failure::Auth;
    }
    const CONFLICT: [&str; 5] = [
        "CONFLICT",
        "Merge conflict",
        "could not apply",
        "needs merge",
        "unmerged files",
    ];
    if CONFLICT.iter().any(|s| has(s)) {
        return Failure::Conflict(Vec::new());
    }
    // "stale info": a lease that no longer holds.
    if has("[rejected]") && (has("non-fast-forward") || has("fetch first") || has("stale info"))
        || has("Updates were rejected")
    {
        return Failure::Rejected;
    }
    let lower = text.to_lowercase();
    if has("Not possible to fast-forward")
        || lower.contains("diverging branches")
        || has("have diverged")
    {
        return Failure::Diverged;
    }
    Failure::Other
}

/// The worktree a branch is checked out in, from git's "'x' is already
/// checked out at '<path>'", "is already used by worktree at '<path>'" or
/// "cannot delete branch 'x' used by worktree at '<path>'".
fn checked_out_at(text: &str) -> Option<PathBuf> {
    let (_, rest) = ["is already checked out at '", "used by worktree at '"]
        .iter()
        .find_map(|s| text.split_once(s))?;
    let line = rest.lines().next()?;
    Some(PathBuf::from(line.rsplit_once('\'')?.0))
}

/// One line on why a command failed: what the remote said when it
/// refused a push, else the last line git marked as an error, else the
/// last line, with `fatal: `, `error: ` or `hint: ` stripped; from
/// stderr, else stdout.
pub(crate) fn why(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    if let Some(said) = refusal(text) {
        return said;
    }
    let lines = || text.lines().map(str::trim).filter(|l| !l.is_empty());
    let marked = lines()
        .rfind(|l| l.starts_with("fatal: ") || l.starts_with("error: "))
        .or_else(|| lines().next_back())
        .unwrap_or("");
    marked
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ")
        .trim_start_matches("hint: ")
        .to_owned()
}

/// Why the remote refused a push, in its words: its `remote:` lines (a
/// hook's, a protected branch's), then the reason `[remote rejected]`
/// gives, `(pre-receive hook declined)`. git's own last line only says
/// "failed to push some refs".
fn refusal(text: &str) -> Option<String> {
    let rejected = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("! [remote rejected]"))?;
    let reason = rejected
        .rsplit_once('(')
        .map(|(_, r)| r.trim().trim_end_matches(')').trim())
        .filter(|r| !r.is_empty());
    // Not the progress a push prints along the way.
    const PROGRESS: [&str; 5] = [
        "Enumerating objects",
        "Counting objects",
        "Compressing objects",
        "Total ",
        "Resolving deltas",
    ];
    let said: Vec<&str> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("remote:"))
        .map(|l| l.trim().trim_start_matches("error: ").trim())
        .filter(|l| !l.is_empty() && !PROGRESS.iter().any(|p| l.starts_with(p)))
        .collect();
    let mut why = said.join(" ");
    if let Some(reason) = reason {
        why = if why.is_empty() {
            reason.to_owned()
        } else {
            format!("{why} ({reason})")
        };
    }
    (!why.is_empty()).then_some(why)
}

/// The files with unresolved conflicts, from the worktree's top folder.
fn conflicted(dir: &Path) -> Vec<String> {
    let out = query(dir, &["diff", "--name-only", "-z", "--diff-filter=U"]);
    out.unwrap_or_default()
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether `args` can run again after it found a lock held: a command
/// that stops before changing anything when it does, or does no harm
/// done twice. Not one that may stop half-way (a rebase, a merge, a
/// stash pop), where trying again would find it in progress.
fn retryable(args: &[&str]) -> bool {
    let mut words = args.iter().copied();
    let verb = loop {
        match words.next() {
            Some("-c" | "-C") => {
                words.next();
            }
            Some(flag) if flag.starts_with('-') => {}
            word => break word,
        }
    };
    match verb {
        Some("fetch" | "push" | "add" | "commit" | "switch" | "branch") => true,
        Some("stash") => matches!(words.next(), Some("push" | "drop")),
        _ => false,
    }
}

/// `Failed` for an operation not tried, for `why`.
fn blocked(why: &str, log: Vec<Ran>) -> Failed {
    Failed {
        failure: Failure::Blocked(why.to_owned()),
        why: why.to_owned(),
        log,
    }
}

/// The commands of one operation in a folder, logged as they run.
struct Op<'a> {
    dir: &'a Path,
    log: Vec<Ran>,
}

impl<'a> Op<'a> {
    fn new(dir: &'a Path) -> Self {
        Self {
            dir,
            log: Vec::new(),
        }
    }

    /// Run local command `args`: its stdout, or the operation's failure.
    fn run(&mut self, args: &[&str]) -> Result<String, Failed> {
        self.exec(args, Reach::Local)
    }

    /// Run `args`, which talks to a remote.
    fn net(&mut self, args: &[&str]) -> Result<String, Failed> {
        self.exec(args, Reach::Network)
    }

    /// Run `args`, again a few times while it finds a lock held, when
    /// running it again is safe; every try is logged.
    fn exec(&mut self, args: &[&str], reach: Reach) -> Result<String, Failed> {
        let mut waits = LOCK_RETRIES.iter().enumerate();
        loop {
            // Stopped: what is left of it isn't run.
            if stopping() {
                return Err(self.blocked(STOPPED));
            }
            let out = exec(self.dir, args, reach);
            self.log.push(out.ran.clone());
            if out.ran.ok {
                return Ok(out.stdout);
            }
            let locked = out.timed_out.is_none()
                && !out.stopped
                && classify(&out.stderr, &out.stdout) == Failure::Locked
                && retryable(args);
            match waits.next() {
                Some((n, &wait)) if locked => {
                    let tries = LOCK_RETRIES.len() + 1;
                    say(&format!("locked: trying again ({} of {tries})…", n + 2));
                    std::thread::sleep(wait);
                }
                _ => return Err(self.failed(&out)),
            }
        }
    }

    /// The failure `out` tells of, with the log so far.
    fn failed(&mut self, out: &Output) -> Failed {
        if out.stopped {
            return self.blocked(STOPPED);
        }
        let log = std::mem::take(&mut self.log);
        if let Some(after) = out.timed_out {
            let why = format!("timed out after {} s", after.as_secs_f32());
            log_failed(self.dir, &log);
            return Failed {
                failure: Failure::Timeout,
                why,
                log,
            };
        }
        let failure = match classify(&out.stderr, &out.stdout) {
            // Conflicted files say so even where the message doesn't.
            Failure::Conflict(_) | Failure::Other => match conflicted(self.dir) {
                files if !files.is_empty() => Failure::Conflict(files),
                _ => classify(&out.stderr, &out.stdout),
            },
            failure => failure,
        };
        let mut why = why(&out.stderr, &out.stdout);
        if why.is_empty() {
            why = format!("{} failed", out.ran.command);
        }
        log_failed(self.dir, &log);
        Failed { failure, why, log }
    }

    fn blocked(&mut self, why: &str) -> Failed {
        blocked(why, std::mem::take(&mut self.log))
    }

    fn done(self) -> OpResult {
        Ok(self.log)
    }
}

/// The top folder of the worktree `dir` is in, which paths are given from;
/// `dir` itself when git can't tell.
fn top(dir: &Path) -> PathBuf {
    query(dir, &["rev-parse", "--show-toplevel"])
        .map(|t| PathBuf::from(t.trim_end_matches('\n')))
        .unwrap_or_else(|| dir.to_owned())
}

/// The checked-out branch; `None` on a detached HEAD.
fn branch(dir: &Path) -> Option<String> {
    query(dir, &["symbolic-ref", "-q", "--short", "HEAD"]).map(|b| b.trim().to_owned())
}

/// The commit checked out; `None` before the first.
fn head_oid(dir: &Path) -> Option<String> {
    query(dir, &["rev-parse", "-q", "--verify", "HEAD^{commit}"]).map(|o| o.trim().to_owned())
}

/// What a diff starts from when there is no commit yet: the empty tree.
fn empty_tree(dir: &Path) -> String {
    let tree = query(dir, &["hash-object", "-t", "tree", "/dev/null"]);
    tree.map(|t| t.trim().to_owned()).unwrap_or_default()
}

/// The repository's remotes.
fn remotes(dir: &Path) -> Vec<String> {
    let out = query(dir, &["remote"]).unwrap_or_default();
    out.lines().map(str::to_owned).collect()
}

/// The remote branches go to when none is given: `origin`, else the only
/// one.
fn default_remote(dir: &Path) -> Option<String> {
    let all = remotes(dir);
    if all.iter().any(|r| r == "origin") {
        return Some("origin".into());
    }
    (all.len() == 1).then(|| all[0].clone())
}

/// The branch `origin` says is its default now, asked over the network: a
/// clone's `origin/HEAD` stays what it was when cloned, and a fetch doesn't
/// move it. `None` with no `origin`, or one that names no branch.
pub fn remote_default(dir: &Path) -> Result<Option<String>, String> {
    if !remotes(dir).iter().any(|r| r == "origin") {
        return Ok(None);
    }
    let out = exec(
        dir,
        &["ls-remote", "--symref", "origin", "HEAD"],
        Reach::Network,
    );
    if !out.ran.ok {
        return Err(out.ran.output.trim().to_owned());
    }
    Ok(out.stdout.lines().find_map(|line| {
        let (head, name) = line.strip_prefix("ref: refs/heads/")?.split_once('\t')?;
        (name == "HEAD").then(|| head.to_owned())
    }))
}

/// The remote that has a branch `name`: `origin` when several do.
fn remote_having(dir: &Path, name: &str) -> Option<String> {
    let refs = query(
        dir,
        &["for-each-ref", "--format=%(refname)", "refs/remotes/"],
    )
    .unwrap_or_default();
    let refs: Vec<&str> = refs.lines().collect();
    let mut having = remotes(dir)
        .into_iter()
        .filter(|r| refs.contains(&format!("refs/remotes/{r}/{name}").as_str()));
    let first = having.next()?;
    Some(if first == "origin" {
        first
    } else {
        having.find(|r| r == "origin").unwrap_or(first)
    })
}

/// The command that checks out branch `name`: the local one, else one new
/// tracking a remote's, else one new from HEAD.
fn switch_args(dir: &Path, name: &str) -> Vec<String> {
    let args = |a: &[&str]| a.iter().map(|&s| s.to_owned()).collect();
    if has_ref(dir, &format!("refs/heads/{name}")) {
        return args(&["switch", name]);
    }
    match remote_having(dir, name) {
        // --track spelt out: `switch name` guesses only with one remote,
        // and not at all with checkout.guess off.
        Some(remote) => args(&["switch", "-c", name, "--track", &format!("{remote}/{name}")]),
        None => args(&["switch", "-c", name]),
    }
}

/// Where `dir`'s checkout stands now.
pub fn head(dir: &Path) -> Option<Head> {
    let sym = exec(dir, &["symbolic-ref", "-q", "--short", "HEAD"], Reach::Read);
    let branch = match sym.code {
        Some(0) => Some(sym.stdout.trim().to_owned()),
        // Not a symbolic ref: detached.
        Some(1) => None,
        _ => return None,
    };
    Some(Head {
        branch,
        oid: head_oid(dir),
    })
}

/// Check out branch `name`: the local one if there is one, else one
/// tracking `<remote>/name` if the remote has it, else a new one from
/// HEAD. Uncommitted changes come along; git refuses when they'd be lost.
pub fn switch_or_create(dir: &Path, name: &str) -> OpResult {
    let mut op = Op::new(dir);
    let args = switch_args(dir, name);
    op.run(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    op.done()
}

/// Commit `paths` (from the worktree's top folder; both paths of a rename)
/// and only them, with message `msg`: `add -A` then `commit --only`, with
/// literal pathspecs. Other files already staged stay staged, out of it.
pub fn commit_paths(dir: &Path, paths: &[String], msg: &str) -> OpResult {
    if paths.is_empty() {
        return Err(blocked("nothing checked", Vec::new()));
    }
    let top = top(dir);
    let mut op = Op::new(&top);
    add_paths(&mut op, &top, paths)?;
    let mut args = vec!["--literal-pathspecs", "commit", "-m", msg, "--only", "--"];
    args.extend(paths.iter().map(String::as_str));
    op.run(&args)?;
    op.done()
}

/// Amend the last commit with `paths` (as `commit_paths` takes them; none
/// to only reword it), and only them: its message replaced by `msg`, or
/// kept without one. Other files already staged stay staged, out of it.
/// No commit yet is `Blocked`.
pub fn amend_paths(dir: &Path, paths: &[String], msg: Option<&str>) -> OpResult {
    let top = top(dir);
    let mut op = Op::new(&top);
    if head_oid(&top).is_none() {
        return Err(op.blocked("no commit to amend"));
    }
    add_paths(&mut op, &top, paths)?;
    let mut args = vec!["--literal-pathspecs", "commit", "--amend"];
    match msg {
        Some(msg) => args.extend(["-m", msg]),
        None => args.push("--no-edit"),
    }
    // `--only` without paths leaves what is staged out: a reword.
    args.push("--only");
    if !paths.is_empty() {
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
    }
    op.run(&args)?;
    op.done()
}

/// Stage `paths` (from the worktree's top folder `top`) for a commit of
/// only them: `add -A` of those on disk or in the index.
fn add_paths(op: &mut Op, top: &Path, paths: &[String]) -> Result<(), Failed> {
    // `add` fails on a path matching nothing on disk or in the index, as
    // the old path of a rename staged with `git mv` is: that one is in
    // HEAD only, which `commit --only` looks in too.
    let listed = {
        let mut args = vec!["--literal-pathspecs", "ls-files", "-z", "--"];
        args.extend(paths.iter().map(String::as_str));
        query(top, &args).unwrap_or_default()
    };
    let indexed: Vec<&str> = listed.split('\0').filter(|p| !p.is_empty()).collect();
    let addable = |path: &&String| {
        let under = |p: &&str| {
            p.strip_prefix(path.trim_end_matches('/'))
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        };
        top.join(path).symlink_metadata().is_ok() || indexed.iter().any(under)
    };
    let add: Vec<&str> = paths.iter().filter(addable).map(String::as_str).collect();
    if !add.is_empty() {
        let mut args = vec!["--literal-pathspecs", "add", "-A", "--"];
        args.extend(add);
        op.run(&args)?;
    }
    Ok(())
}

/// Push the branch: plain `git push` when its upstream is the same-named
/// branch, else `push -u <remote> HEAD`, to `remote` or else the
/// upstream's, `origin` or the only one. `force_lease` pushes with
/// `--force-with-lease`. A detached HEAD or no remote is `Blocked`.
pub fn push(dir: &Path, remote: Option<&str>, force_lease: bool) -> OpResult {
    let mut op = Op::new(dir);
    let Some(branch) = branch(dir) else {
        return Err(op.blocked("detached HEAD"));
    };
    let upstream = query(
        dir,
        &[
            "for-each-ref",
            "--format=%(upstream:remotename) %(upstream:remoteref)",
            &format!("refs/heads/{branch}"),
        ],
    )
    .unwrap_or_default();
    let (up_remote, up_ref) = upstream.trim().split_once(' ').unwrap_or(("", ""));
    // "." is a local branch's upstream: no remote.
    let up_remote = (!up_remote.is_empty() && up_remote != ".").then_some(up_remote);
    // The lease alone protects nothing once a fetch moved the remote's
    // branch, as the panel's own fetches do: the remote's commits must be
    // in this one too.
    let lease: &[&str] = if force_lease {
        &["--force-with-lease", "--force-if-includes"]
    } else {
        &[]
    };
    // Named in full, so a push.default or pushDefault of the user's can't
    // push other branches, or elsewhere.
    if let Some(up_remote) = up_remote.filter(|_| up_ref == format!("refs/heads/{branch}")) {
        let target = format!("HEAD:{up_ref}");
        let args: Vec<&str> = ["push"]
            .into_iter()
            .chain(lease.iter().copied())
            .chain([up_remote, target.as_str()])
            .collect();
        op.net(&args)?;
        return op.done();
    }
    let remote = remote
        .map(str::to_owned)
        .or(up_remote.map(str::to_owned))
        .or_else(|| default_remote(dir));
    let Some(remote) = remote else {
        return Err(op.blocked("no remote"));
    };
    let args: Vec<&str> = ["push"]
        .into_iter()
        .chain(lease.iter().copied())
        .chain(["-u", &remote, "HEAD"])
        .collect();
    op.net(&args)?;
    op.done()
}

/// Start merging `base` into the checked-out branch, `--no-edit`: the files
/// it stops on, empty when it merged cleanly. Uncommitted changes in the
/// way make git refuse.
pub fn begin_merge(dir: &Path, base: &str) -> Result<Vec<String>, Failed> {
    let mut op = Op::new(dir);
    if head_oid(dir).is_none() {
        return Err(op.blocked("no commits yet"));
    }
    if op
        .run(&["merge", "--no-edit", "--no-autostash", base])
        .is_ok()
    {
        return Ok(Vec::new());
    }
    let unmerged = query(dir, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    let files: Vec<String> = unmerged.lines().map(str::to_owned).collect();
    if files.is_empty() {
        return Err(op.blocked("the merge stopped, but on no conflicted file"));
    }
    Ok(files)
}

/// `push <remote> --delete <branch>`: the branch goes off the remote, with
/// `expect` only while it is at that commit.
pub fn delete_remote_branch(
    dir: &Path,
    remote: &str,
    branch: &str,
    expect: Option<&str>,
) -> OpResult {
    let mut op = Op::new(dir);
    let lease = expect.map(|sha| format!("--force-with-lease={branch}:{sha}"));
    let args: Vec<&str> = ["push"]
        .into_iter()
        .chain(lease.as_deref())
        .chain([remote, "--delete", branch])
        .collect();
    op.net(&args)?;
    op.done()
}

/// `fetch <remote> <branch>`: that branch's remote-tracking ref only.
pub fn fetch_branch(dir: &Path, remote: &str, branch: &str) -> OpResult {
    let mut op = Op::new(dir);
    op.net(&["fetch", "--quiet", remote, branch])?;
    op.done()
}

/// `fetch --all --prune`.
pub fn fetch_all(dir: &Path) -> OpResult {
    let mut op = Op::new(dir);
    op.net(&["fetch", "--all", "--prune"])?;
    op.done()
}

/// Pull the upstream's commits, `how`. Uncommitted changes it would touch
/// make git refuse, leaving all as it was: not stashed around it, since
/// a stash that conflicts coming back would read as done.
pub fn pull(dir: &Path, how: Pull) -> OpResult {
    let mut op = Op::new(dir);
    if branch(dir).is_none() {
        return Err(op.blocked("detached HEAD"));
    }
    if query(dir, &["rev-parse", "-q", "--verify", "@{upstream}"]).is_none() {
        return Err(op.blocked("no upstream"));
    }
    // --no-rebase too, so a pull.rebase of the user's doesn't win.
    let args: &[&str] = match how {
        Pull::FastForward => &["pull", "--no-rebase", "--ff-only"],
        Pull::Rebase => &["pull", "--rebase", "--no-autostash"],
        Pull::Merge => &["pull", "--no-rebase", "--no-edit", "--no-autostash"],
    };
    op.net(args)?;
    op.done()
}

/// Bring `base` (`origin/main`) into the checked-out branch, `how`; a
/// merge with `--no-edit`. Uncommitted changes in the way make git refuse
/// (see `pull`). Blocked before the first commit.
pub fn integrate(dir: &Path, base: &str, how: Integrate) -> OpResult {
    let mut op = Op::new(dir);
    if head_oid(dir).is_none() {
        return Err(op.blocked("no commits yet"));
    }
    match how {
        Integrate::Merge => op.run(&["merge", "--no-edit", "--no-autostash", base])?,
        Integrate::Rebase => op.run(&["rebase", "--no-autostash", base])?,
    };
    op.done()
}

/// Mark a conflicted file resolved: `git add` (a deleted one's removal
/// too).
pub fn mark_resolved(dir: &Path, path: &str) -> OpResult {
    let top = top(dir);
    let mut op = Op::new(&top);
    op.run(&["--literal-pathspecs", "add", "-A", "--", path])?;
    op.done()
}

/// Carry on with `op` after its conflicts are resolved; a rebase may stop
/// on the next commit's (`Failure::Conflict` again).
pub fn continue_op(dir: &Path, op: InProgress) -> OpResult {
    let args: &[&str] = match op {
        InProgress::Merge => &["commit", "--no-edit"],
        InProgress::Rebase { .. } => &["rebase", "--continue"],
        InProgress::CherryPick => &["cherry-pick", "--continue"],
        InProgress::Revert => &["revert", "--continue"],
    };
    let mut op = Op::new(dir);
    op.run(args)?;
    op.done()
}

/// Abort `op`, back to where it started.
pub fn abort_op(dir: &Path, op: InProgress) -> OpResult {
    let args: &[&str] = match op {
        InProgress::Merge => &["merge", "--abort"],
        InProgress::Rebase { .. } => &["rebase", "--abort"],
        InProgress::CherryPick => &["cherry-pick", "--abort"],
        InProgress::Revert => &["revert", "--abort"],
    };
    let mut op = Op::new(dir);
    op.run(args)?;
    op.done()
}

/// Undo a batch, only while HEAD is still at `after` (else `Blocked("HEAD
/// moved since")`): back to `before`'s branch if it differs, else to its
/// commit; `soft` keeps what the commits undone changed as staged changes
/// (undoing a commit), else they go (`reset --keep`: undoing a merge, a
/// rebase or a pull, whose commits the remote still has).
pub fn undo(dir: &Path, before: &Head, after: &Head, soft: bool) -> OpResult {
    let mut op = Op::new(dir);
    if head(dir).as_ref() != Some(after) {
        return Err(op.blocked("HEAD moved since"));
    }
    let Some(oid) = &before.oid else {
        return Err(op.blocked("no commit to go back to"));
    };
    // A switch is undone by switching back, and only that: the branch
    // left may have moved on since, in another worktree.
    if before.branch != after.branch {
        match &before.branch {
            Some(branch) => op.run(&["switch", branch])?,
            None => op.run(&["switch", "--detach", oid])?,
        };
        return op.done();
    }
    op.run(&["reset", if soft { "--soft" } else { "--keep" }, oid])?;
    op.done()
}

/// How many commits of local branch `name` no other branch, remote branch
/// or tag has: what deleting it loses. Its own copy on a remote doesn't
/// count: a fetch that prunes takes that away once the remote deleted it.
/// Zero when its changes are all in a default branch already, squashed or
/// rebased there (see `git::merged`). `None` when git can't tell.
pub fn unmerged_commits(dir: &Path, name: &str) -> Option<usize> {
    let exclude = format!("--exclude={name}");
    let remote_copy = format!("--exclude=*/{name}");
    let args = [
        "rev-list",
        "--count",
        &format!("refs/heads/{name}"),
        "--not",
        &exclude,
        "--branches",
        &remote_copy,
        "--remotes",
        "--tags",
    ];
    let count = query(dir, &args)?.trim().parse().ok()?;
    if count > 0 && git::merged(dir, &format!("refs/heads/{name}"), &[]) {
        return Some(0);
    }
    Some(count)
}

/// Delete local branch `name`; unless `force`, only when that loses no
/// commit (see `unmerged_commits`): else `Blocked`.
pub fn delete_branch(dir: &Path, name: &str, force: bool) -> OpResult {
    let mut op = Op::new(dir);
    if !force {
        match unmerged_commits(dir, name) {
            Some(0) => {}
            Some(1) => return Err(op.blocked("1 commit in no other branch")),
            Some(n) => return Err(op.blocked(&format!("{n} commits in no other branch"))),
            None => return Err(op.blocked("can't tell what it would lose")),
        }
    }
    op.run(&["branch", "-D", name])?;
    op.done()
}

/// Whether merging `theirs` into `ours` would conflict, without touching
/// the worktree (`merge-tree --write-tree --name-only`).
pub fn predict_merge(dir: &Path, ours: &str, theirs: &str) -> Verdict {
    let args = [
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        "-z",
        ours,
        theirs,
    ];
    let out = exec(dir, &args, Reach::Read);
    // The tree it made, then (with -z) a NUL after each conflicted file.
    let mut fields = out.stdout.split('\0');
    let tree = fields.next().unwrap_or("");
    let made = !tree.is_empty() && tree.chars().all(|c| c.is_ascii_hexdigit());
    match out.code {
        Some(0) if made => Verdict::Clean,
        // Exit 1 is also a bad name: no tree then.
        Some(1) if made => Verdict::Conflicts(
            fields
                .take_while(|f| !f.is_empty())
                .map(str::to_owned)
                .collect(),
        ),
        _ if out.timed_out.is_some() => Verdict::Blocked("timed out".into()),
        _ => {
            let why = why(&out.stderr, &out.stdout);
            Verdict::Blocked(if why.is_empty() {
                "merge-tree failed".into()
            } else {
                why
            })
        }
    }
}

/// Of the uncommitted files `dirty`, those that differ between HEAD and
/// `target`: what keeps git from switching there with them. `target` is
/// a branch (local or a remote's) or any commit; none when there is no
/// such.
pub fn switch_blockers(dir: &Path, target: &str, dirty: &[String]) -> Vec<String> {
    if dirty.is_empty() {
        return Vec::new();
    }
    let top = top(dir);
    let target = if has_ref(&top, &format!("refs/heads/{target}")) {
        format!("refs/heads/{target}")
    } else if let Some(remote) = remote_having(&top, target) {
        format!("refs/remotes/{remote}/{target}")
    } else if has_ref(&top, target) {
        target.to_owned()
    } else {
        return Vec::new();
    };
    let from = head_oid(&top).unwrap_or_else(|| empty_tree(&top));
    let mut args = vec![
        "--literal-pathspecs",
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        &from,
        &target,
        "--",
    ];
    args.extend(dirty.iter().map(String::as_str));
    let out = query(&top, &args).unwrap_or_default();
    let differ: Vec<&str> = out.split('\0').filter(|p| !p.is_empty()).collect();
    dirty
        .iter()
        .filter(|d| {
            differ
                .iter()
                .any(|f| f == d || (d.ends_with('/') && f.starts_with(d.as_str())))
        })
        .cloned()
        .collect()
}

/// The first of `names` the repository at `dir` has as a branch, else the
/// first `remote` has: what a switch to them goes to there.
pub fn branch_named(dir: &Path, names: &[String], remote: Option<&str>) -> Option<String> {
    let local = names
        .iter()
        .find(|name| has_ref(dir, &format!("refs/heads/{name}")));
    let theirs = || {
        let remote = remote?;
        names
            .iter()
            .find(|name| has_ref(dir, &format!("refs/remotes/{remote}/{name}")))
    };
    local.or_else(theirs).cloned()
}

/// What a merge or rebase from the branch `names` (one branch, as `main`
/// and `master`) brings in at `dir`: the first of them it has as a branch,
/// else the first `remote` has (`origin/dev`). With both, the remote's when
/// it is ahead (fetched, not pulled yet), else the local one, which may
/// have commits not pushed.
pub fn source_of(dir: &Path, names: &[String], remote: Option<&str>) -> Option<String> {
    let local = names
        .iter()
        .find(|name| has_ref(dir, &format!("refs/heads/{name}")));
    let theirs = remote.and_then(|remote| {
        names
            .iter()
            .map(|name| format!("{remote}/{name}"))
            .find(|t| has_ref(dir, &format!("refs/remotes/{t}")))
    });
    match (local, theirs) {
        (Some(local), Some(theirs)) => {
            let within =
                |a: &str, b: &str| query(dir, &["merge-base", "--is-ancestor", a, b]).is_some();
            let behind = within(local, &theirs) && !within(&theirs, local);
            Some(if behind { theirs } else { local.clone() })
        }
        (local, theirs) => local.cloned().or(theirs),
    }
}

/// Whether ref `name` (`refs/heads/x`, `refs/remotes/origin/x`) exists;
/// any other name, whether it names a commit.
pub fn has_ref(dir: &Path, name: &str) -> bool {
    if name.starts_with("refs/") {
        return query(dir, &["show-ref", "--verify", "-q", name]).is_some();
    }
    if name.starts_with('-') {
        return false;
    }
    query(
        dir,
        &["rev-parse", "-q", "--verify", &format!("{name}^{{commit}}")],
    )
    .is_some()
}

/// Whether `name` is a valid branch name (`check-ref-format --branch`).
/// Only tests ask git: the panel checks names itself
/// (`repos::branch_name_problem`), against this.
#[cfg(test)]
pub fn valid_branch(dir: &Path, name: &str) -> bool {
    // `--branch` would take `@{-1}` for the branch before; `HEAD` and a
    // leading dash would read as something else.
    if name.is_empty() || name.starts_with('-') || name == "HEAD" || name.contains("@{") {
        return false;
    }
    query(dir, &["check-ref-format", "--branch", name]).is_some()
}

/// `text` cut to `lines` lines, saying how many more there were.
fn cap(text: String, lines: usize) -> String {
    let total = text.lines().count();
    if total <= lines {
        return text;
    }
    let mut out: String = text.lines().take(lines).flat_map(|l| [l, "\n"]).collect();
    out.push_str(&format!("… {} more lines\n", total - lines));
    out
}

/// The changes of a repository's `changes` against HEAD, one diff per
/// change in their order, for the detail pane: the tracked ones from one
/// `git diff`, an untracked file as all added, an untracked folder as
/// its files.
pub fn repo_diff(dir: &Path, changes: &[Change]) -> Vec<FileDiff> {
    repo_diff_from(dir, changes, None)
}

/// `repo_diff` against commit `from` (HEAD for none).
pub fn repo_diff_from(dir: &Path, changes: &[Change], from: Option<&str>) -> Vec<FileDiff> {
    let top = top(dir);
    let mut tracked = Vec::new();
    let mut failed = None;
    if changes.iter().any(|c| c.code != '?') {
        let from = match from {
            Some(from) => from.to_owned(),
            None => head_oid(&top).unwrap_or_else(|| empty_tree(&top)),
        };
        let args = [
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-M",
            &from,
        ];
        let out = exec(&top, &args, Reach::Read);
        match out.code {
            Some(0) => tracked = diff::parse(&cap(out.stdout, REPO_DIFF_LINES)),
            _ => failed = Some(why(&out.stderr, &out.stdout)),
        }
    }
    changes
        .iter()
        .map(|change| match change.code {
            '?' => untracked(&top, change),
            _ => match_file(&mut tracked, change, failed.as_deref()),
        })
        .collect()
}

/// What file `path` changed from blob `from` to blob `to` of the repository
/// at `dir`: what a review shows of it since it was viewed on `from`. None
/// when git can't say.
pub fn blob_diff(dir: &Path, from: &str, to: &str, path: &str) -> Option<FileDiff> {
    let args = [
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        from,
        to,
    ];
    let out = exec(&top(dir), &args, Reach::Read);
    // The review then shows the file's whole diff: nothing is lost.
    if out.code != Some(0) {
        tracing::warn!("diff of {path} since viewed: {}", why(&out.stderr, &out.stdout));
        return None;
    }
    let mut file = diff::parse(&cap(out.stdout, REPO_DIFF_LINES))
        .into_iter()
        .next()?;
    file.path = path.to_owned();
    file.code = 'M';
    Some(file)
}

/// The diff of each of `changes` out of the files one `git diff` printed,
/// in their order; a change it hasn't says why (`failed`, else that
/// there is nothing to show).
fn match_files(
    mut tracked: Vec<FileDiff>,
    changes: &[Change],
    failed: Option<&str>,
) -> Vec<FileDiff> {
    changes
        .iter()
        .map(|change| match_file(&mut tracked, change, failed))
        .collect()
}

/// `change`'s diff, taken out of `tracked` (see `match_files`).
fn match_file(tracked: &mut Vec<FileDiff>, change: &Change, failed: Option<&str>) -> FileDiff {
    let found = tracked.iter().position(|f| f.path == change.path);
    let mut file = found.map_or_else(FileDiff::default, |i| tracked.swap_remove(i));
    if found.is_none() {
        file.notes
            .push(failed.unwrap_or("No changes to show").to_owned());
    }
    file.path = change.path.clone();
    file.orig = change.orig.clone();
    file.code = change.code;
    file
}

/// What a worktree changed since it forked from `target` (since HEAD for
/// none, or before its first commit): the commit it forked at, how many
/// commits it made since, and every file changed since, committed or not,
/// then the untracked ones (a folder untracked whole as one, ending in
/// `/`); else why they can't be read.
pub fn changes_since(dir: &Path, target: Option<&str>) -> Result<Since, String> {
    let top = top(dir);
    let head = head_oid(&top);
    let base = match (target, &head) {
        (Some(target), Some(_)) => query(&top, &["merge-base", target, "HEAD"])
            .map(|b| b.trim().to_owned())
            .ok_or_else(|| format!("It has no commit in common with {target}"))?,
        (None, Some(head)) => head.clone(),
        (_, None) => empty_tree(&top),
    };
    let args = [
        "-c",
        "core.quotePath=false",
        "diff",
        "--name-status",
        "-z",
        "-M",
        "--no-ext-diff",
        &base,
    ];
    let out = exec(&top, &args, Reach::Read);
    if out.code != Some(0) {
        return Err(why(&out.stderr, &out.stdout));
    }
    let mut changes = parse_name_status(&out.stdout);
    let args = [
        "ls-files",
        "-z",
        "--others",
        "--exclude-standard",
        "--directory",
        "--no-empty-directory",
    ];
    let out = exec(&top, &args, Reach::Read);
    if out.code != Some(0) {
        return Err(why(&out.stderr, &out.stdout));
    }
    changes.extend(
        out.stdout
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|path| Change {
                path: path.to_owned(),
                orig: None,
                code: '?',
            }),
    );
    let commits = match &head {
        Some(_) => query(&top, &["rev-list", "--count", &format!("{base}..HEAD")])
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0),
        None => 0,
    };
    Ok(Since {
        base,
        commits,
        changes,
    })
}

/// What a worktree changed since it forked (`changes_since`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Since {
    /// The commit it forked at, which its diff is against.
    pub base: String,
    /// Its commits since.
    pub commits: usize,
    pub changes: Vec<Change>,
}

/// The files `git diff --name-status -z` lists: what happened to each, by
/// its letter, a rename or copy with where it was.
fn parse_name_status(out: &str) -> Vec<Change> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
        let code = status.chars().next().unwrap_or('M');
        let (orig, path) = match code {
            'R' | 'C' => (fields.next().map(str::to_owned), fields.next()),
            _ => (None, fields.next()),
        };
        let Some(path) = path else {
            break;
        };
        changes.push(Change {
            path: path.to_owned(),
            orig,
            code,
        });
    }
    changes
}

/// An untracked file as a diff adding all of it, an untracked folder as
/// the files in it; or why they aren't shown.
fn untracked(top: &Path, change: &Change) -> FileDiff {
    let path = change.path.as_str();
    let note = |note: String| FileDiff {
        path: path.into(),
        code: change.code,
        notes: vec![note],
        ..FileDiff::default()
    };
    if path.ends_with('/') {
        let args = [
            "--literal-pathspecs",
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            path,
        ];
        let out = exec(top, &args, Reach::Read);
        if out.code != Some(0) {
            return note(why(&out.stderr, &out.stdout));
        }
        let mut file = note(String::new());
        let all: Vec<&str> = out.stdout.lines().collect();
        file.notes = all
            .iter()
            .take(FOLDER_FILES)
            .map(|l| l.to_string())
            .collect();
        if all.len() > FOLDER_FILES {
            file.notes
                .push(format!("… {} more files", all.len() - FOLDER_FILES));
        }
        return file;
    }
    let full = top.join(path);
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(meta) => meta,
        Err(e) => return note(e.to_string()),
    };
    if meta.file_type().is_symlink() {
        let to = std::fs::read_link(&full).unwrap_or_default();
        return note(format!("Symbolic link to {}", to.display()));
    }
    if meta.len() > UNTRACKED_BYTES {
        return note(format!("Not shown: {} KB", meta.len() / 1024));
    }
    let bytes = match std::fs::read(&full) {
        Ok(bytes) => bytes,
        Err(e) => return note(e.to_string()),
    };
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return note("Binary file".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    if text.is_empty() {
        return note("Empty file".into());
    }
    let mut diff = String::from("diff --git a/f b/f\n@@ -0,0 +1 @@\n");
    for line in text.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    let mut file = diff::parse(&diff).pop().unwrap_or_default();
    file.path = path.into();
    file.code = change.code;
    diff::colour_file(&mut file);
    file
}

/// One commit of a log: its full id, and the line shown for it (`1a2b3c4
/// subject · author, 2 hours ago`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub line: String,
}

/// The Log tab's lists, each under a line saying what it is: the commits
/// to pull and the ones to push, else the last few.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitLog {
    pub sections: Vec<(String, Vec<Commit>)>,
}

impl CommitLog {
    /// Its lines, a blank one between lists, each with the commit it
    /// shows (none for the others).
    pub fn lines(&self) -> Vec<(String, Option<String>)> {
        let mut lines = Vec::new();
        for (i, (header, commits)) in self.sections.iter().enumerate() {
            if i > 0 {
                lines.push((String::new(), None));
            }
            lines.push((header.clone(), None));
            for c in commits {
                lines.push((c.line.clone(), Some(c.oid.clone())));
            }
        }
        lines
    }
}

/// The commits git log `args` lists, newest first, at most `LOG_MAX`.
fn log_commits(dir: &Path, args: &[&str]) -> Vec<Commit> {
    let format = "--format=%H%x1f%h%x1f%s%x1f%an%x1f%cr";
    let n = LOG_MAX.to_string();
    let base = ["log", "--no-color", format, "-n", &n];
    let out = query(dir, &[&base[..], args].concat()).unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\x1f');
            let (oid, short) = (f.next()?, f.next()?);
            let subject = f.next().unwrap_or_default();
            let author = f.next().unwrap_or_default();
            let age = f.next().unwrap_or_default();
            Some(Commit {
                oid: oid.to_owned(),
                line: format!("{short} {subject} · {author}, {age}"),
            })
        })
        .collect()
}

/// How many commits `range` holds; none when it can't be read.
pub fn count_commits(dir: &Path, range: &str) -> usize {
    query(dir, &["rev-list", "--count", range])
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

/// The commits in `range` under `header` and how many there are; past
/// `LOG_MAX`, it says only the newest are listed.
fn log_section(dir: &Path, range: &str, header: String) -> (String, Vec<Commit>) {
    let count = count_commits(dir, range);
    if count == 0 {
        return (format!("{header}: none"), Vec::new());
    }
    let commits = log_commits(dir, &[range]);
    let header = match commits.len() {
        0 => format!("{header} ({count}): they couldn't be read"),
        shown if shown < count => format!("{header} ({count}, the newest {shown}):"),
        _ => format!("{header} ({count}):"),
    };
    (header, commits)
}

/// The commits to pull from the upstream and the ones to push to it, each
/// under a line saying so (`To pull from origin/main (2):`); with neither,
/// or without an upstream, the last few.
pub fn log_upstream(dir: &Path) -> CommitLog {
    let last = |header: &str| CommitLog {
        sections: vec![(header.to_owned(), log_commits(dir, &["-n", LOG_LAST]))],
    };
    let Some(upstream) = query(dir, &["rev-parse", "-q", "--abbrev-ref", "@{upstream}"]) else {
        return last("No upstream yet. The last commits:");
    };
    let upstream = upstream.trim();
    if count_commits(dir, "@{upstream}...HEAD") == 0 {
        return last(&format!("Up to date with {upstream}. The last commits:"));
    }
    let pull = log_section(dir, "HEAD..@{upstream}", format!("To pull from {upstream}"));
    let push = log_section(dir, "@{upstream}..HEAD", format!("To push to {upstream}"));
    CommitLog {
        sections: vec![pull, push],
    }
}

/// The first `n` commits of `range`, newest first, as their short id and
/// subject, for a tooltip.
pub fn commits_between(dir: &Path, range: &str, n: usize) -> Vec<(String, String)> {
    let n = n.to_string();
    let args = ["log", "--no-color", "--format=%h%x1f%s", "-n", &n, range];
    let out = query(dir, &args).unwrap_or_default();
    out.lines()
        .filter_map(|l| l.split_once('\x1f'))
        .map(|(h, s)| (h.to_owned(), s.to_owned()))
        .collect()
}

/// What commit `oid` changed against its first parent (the empty tree for
/// a first commit): its files, each with its diff; else why it can't be
/// read.
pub fn commit_diff(dir: &Path, oid: &str) -> Result<(Vec<Change>, Vec<FileDiff>), String> {
    let top = top(dir);
    let parent = format!("{oid}^");
    let from = query(
        &top,
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("{parent}^{{commit}}"),
        ],
    )
    .map(|p| p.trim().to_owned())
    .unwrap_or_else(|| empty_tree(&top));
    range_diff(&top, &from, oid)
}

/// What the commits from `from` to `to` changed: their files, each with
/// its diff; else why they can't be read.
pub fn range_diff(
    dir: &Path,
    from: &str,
    to: &str,
) -> Result<(Vec<Change>, Vec<FileDiff>), String> {
    let top = top(dir);
    let diff = [
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
    ];
    let out = exec(
        &top,
        &[&diff[..], &["--name-status", "-z", "-M", from, to]].concat(),
        Reach::Read,
    );
    if out.code != Some(0) {
        return Err(why(&out.stderr, &out.stdout));
    }
    let changes = parse_name_status(&out.stdout);
    let out = exec(&top, &[&diff[..], &["-M", from, to]].concat(), Reach::Read);
    if out.code != Some(0) {
        return Err(why(&out.stderr, &out.stdout));
    }
    let tracked = diff::parse(&cap(out.stdout, REPO_DIFF_LINES));
    let files = match_files(tracked, &changes, None);
    Ok((changes, files))
}

/// The repository's stashes, newest first, one line each: `stash@{0} ·
/// 2 hours ago · WIP on main: 1a2b3c4 msg`; else a line saying there are
/// none. With each line's stash commit.
pub fn stash_list(dir: &Path) -> (String, Vec<Option<String>>) {
    let list = query(dir, &["stash", "list", "--format=%H %gd · %cr · %gs"]).unwrap_or_default();
    if list.trim().is_empty() {
        return ("No stashes".into(), Vec::new());
    }
    let (oids, lines): (Vec<_>, Vec<_>) = (list.lines())
        .map(|l| l.split_once(' ').unwrap_or(("", l)))
        .map(|(oid, line)| (Some(oid.to_owned()), line))
        .unzip();
    (lines.join("\n"), oids)
}

/// Throw away the uncommitted changes to `paths` (from the worktree's top
/// folder): a file HEAD has goes back to it, index and all; one it
/// hasn't (untracked, or only added) is deleted, a folder of them too.
pub fn discard(dir: &Path, paths: &[String]) -> OpResult {
    let top = top(dir);
    let mut op = Op::new(&top);
    for path in paths {
        let path = path.trim_end_matches('/');
        let in_head = query(&top, &["cat-file", "-e", &format!("HEAD:{path}")]).is_some();
        if in_head {
            let restore = ["--literal-pathspecs", "restore", "--source=HEAD"];
            op.run(&[&restore[..], &["--staged", "--worktree", "--", path]].concat())?;
        } else {
            let rm = ["--literal-pathspecs", "rm", "-r", "-q", "--cached"];
            op.run(&[&rm[..], &["--ignore-unmatch", "--", path]].concat())?;
            op.run(&["--literal-pathspecs", "clean", "-f", "-d", "-q", "--", path])?;
        }
    }
    op.done()
}

/// Stash the uncommitted changes, untracked files too.
pub fn stash_push(dir: &Path) -> OpResult {
    let mut op = Op::new(dir);
    op.run(&["stash", "push", "--include-untracked"])?;
    op.done()
}

/// Bring stash `n` (0 the newest) back and drop it, only while it is still
/// the one `seen`; a conflict keeps the stash.
pub fn stash_pop(dir: &Path, n: usize, seen: &StashSeen) -> OpResult {
    let mut op = Op::new(dir);
    if !stash_unchanged(dir, n, seen) {
        return Err(op.blocked(STASH_CHANGED));
    }
    op.run(&["stash", "pop", &format!("stash@{{{n}}}")])?;
    op.done()
}

/// Drop stash `n` (0 the newest); only while it is still the one `seen`.
pub fn stash_drop(dir: &Path, n: usize, seen: &StashSeen) -> OpResult {
    let mut op = Op::new(dir);
    let name = format!("stash@{{{n}}}");
    if !stash_unchanged(dir, n, seen) {
        return Err(op.blocked(STASH_CHANGED));
    }
    op.run(&["stash", "drop", &name])?;
    op.done()
}

const STASH_CHANGED: &str = "the stash list changed; refresh and try again";

/// Whether stash `n` is still the one `seen`: a stash made or dropped
/// since shifts every `n` by one.
fn stash_unchanged(dir: &Path, n: usize, seen: &StashSeen) -> bool {
    match seen {
        StashSeen::Oid(oid) => stash_oid(dir, n).as_deref() == Some(oid.as_str()),
        StashSeen::Count(count) => query(dir, &["stash", "list"])
            .is_some_and(|list| list.lines().count() == *count as usize),
    }
}

/// What a stash drop saw of the stash list, so that it refuses once the
/// list changed: the stash's commit, or (no list read) how many there were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StashSeen {
    Oid(String),
    Count(u32),
}

/// The commit id of `stash@{n}` now; `None` when there is none.
pub fn stash_oid(dir: &Path, n: usize) -> Option<String> {
    let name = format!("stash@{{{n}}}");
    query(dir, &["rev-parse", "--verify", "-q", &name]).map(|oid| oid.trim().to_owned())
}

/// Who commits, as git's `user.name` and `user.email` have it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// What is wrong with it for git, as a sentence; `None` when git
    /// takes it.
    pub fn problem(&self) -> Option<&'static str> {
        let (name, email) = (self.name.trim(), self.email.trim());
        let bad = |s: &str| s.contains(['<', '>', '\n']);
        if name.is_empty() {
            Some("Type the name your commits carry.")
        } else if bad(name) {
            Some("A name can't have < or > in it.")
        } else if email.is_empty() {
            Some("Type the email your commits carry.")
        } else if bad(email) || email.contains(char::is_whitespace) || !email.contains('@') {
            Some("That email doesn't look right: it needs an @ and no spaces.")
        } else {
            None
        }
    }
}

/// Who commits in `dir` as its config says (its own, the user's, the
/// system's): empty where none is set.
pub fn identity(dir: &Path) -> Identity {
    let get = |key: &str| {
        query(dir, &["config", "--get", key])
            .map(|v| v.trim().to_owned())
            .unwrap_or_default()
    };
    Identity {
        name: get("user.name"),
        email: get("user.email"),
    }
}

/// A guess at who commits in `roots`, where git knows nobody: what their
/// config has (a name with no email), else what one of `others` has,
/// else the GitHub account gh is signed in to (its noreply address when
/// the email is private). Empty where nothing tells.
pub fn guess_identity(roots: &[PathBuf], others: &[PathBuf]) -> Identity {
    let fill = |guess: &mut Identity, found: Identity| {
        if guess.name.is_empty() {
            guess.name = found.name;
        }
        if guess.email.is_empty() {
            guess.email = found.email;
        }
    };
    let mut guess = Identity::default();
    for dir in roots.iter().chain(others) {
        fill(&mut guess, identity(dir));
    }
    if (guess.name.is_empty() || guess.email.is_empty())
        && let Some(dir) = roots.first()
    {
        // Only a guess: without gh, or signed out, the user types it.
        match crate::ci::gh_json::<serde_json::Value>(dir, &["api", "user"]) {
            Ok(user) => fill(&mut guess, github_identity(&user)),
            Err(why) => tracing::debug!("guessing a git identity from gh: {why}"),
        }
    }
    guess
}

/// Who the GitHub account `user` (`gh api user`) commits as: its name,
/// else its login; its public email, else its noreply address.
fn github_identity(user: &serde_json::Value) -> Identity {
    let text = |key: &str| {
        user.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let login = text("login");
    let email = text("email").map(str::to_owned).or_else(|| {
        let id = user.get("id")?.as_u64()?;
        Some(format!("{id}+{}@users.noreply.github.com", login?))
    });
    Identity {
        name: text("name").or(login).unwrap_or_default().to_owned(),
        email: email.unwrap_or_default(),
    }
}

/// Make `who` the one committing: for every repository when `global`
/// (the user's git config), else in each of `roots`. Why it failed, if
/// it did.
pub fn set_identity(roots: &[PathBuf], global: bool, who: &Identity) -> Result<(), String> {
    let pairs = [
        ("user.name", who.name.trim()),
        ("user.email", who.email.trim()),
    ];
    let home = crate::home_dir();
    let dirs: Vec<&Path> = match global {
        true => home
            .as_deref()
            .or(roots.first().map(PathBuf::as_path))
            .into_iter()
            .collect(),
        false => roots.iter().map(PathBuf::as_path).collect(),
    };
    if dirs.is_empty() {
        return Err("no repository to set it in".into());
    }
    for dir in dirs {
        for (key, value) in pairs {
            let mut args = vec!["config"];
            if global {
                args.push("--global");
            }
            args.extend([key, value]);
            let out = exec(dir, &args, Reach::Local);
            if !out.ran.ok {
                log_failed(dir, std::slice::from_ref(&out.ran));
                let why = why(&out.stderr, &out.stdout);
                return Err(if why.is_empty() {
                    format!("{} failed", out.ran.command)
                } else {
                    why
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::LineKind;
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicUsize, Ordering},
    };

    /// Run git in `dir` as the operations do, which it must survive: what
    /// it printed.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = exec(dir, args, Reach::Local);
        assert!(out.ran.ok, "{}: {}", out.ran.command, out.ran.output);
        out.stdout
    }

    /// The commit `rev` names in `dir`.
    fn rev(dir: &Path, rev: &str) -> String {
        git(dir, &["rev-parse", rev]).trim().to_owned()
    }

    /// Write `text` to `path` under `dir`, its folders too.
    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(dir: &Path, path: &str) -> String {
        std::fs::read_to_string(dir.join(path)).unwrap()
    }

    /// Write `file` in `dir` as `text` and commit it, with `text` for the
    /// message.
    fn commit(dir: &Path, file: &str, text: &str) {
        write(dir, file, text);
        git(dir, &["add", "--", file]);
        git(dir, &["commit", "-q", "-m", text]);
    }

    fn failure(result: OpResult) -> Failure {
        result.expect_err("should have failed").failure
    }

    /// A folder of its own under the system's temporary folder, removed
    /// when dropped: a repository `repo` on branch main with a first
    /// commit, and (but for `local`) a bare `origin.git` it pushed main
    /// to, upstream set.
    struct Fx {
        root: PathBuf,
        repo: PathBuf,
        origin: PathBuf,
    }

    impl Fx {
        fn local() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "gerdr-git-ops-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("repo")).unwrap();
            let root = root.canonicalize().unwrap();
            let fx = Self {
                repo: root.join("repo"),
                origin: root.join("origin.git"),
                root,
            };
            git(&fx.repo, &["init", "-q", "-b", "main"]);
            commit(&fx.repo, "a", "first");
            fx
        }

        fn new() -> Self {
            let fx = Self::local();
            git(
                &fx.root,
                &["init", "-q", "--bare", "-b", "main", "origin.git"],
            );
            let origin = fx.origin.to_str().unwrap();
            git(&fx.repo, &["remote", "add", "origin", origin]);
            git(&fx.repo, &["push", "-q", "-u", "origin", "main"]);
            fx
        }

        /// Another clone of origin, at `name`: someone else's.
        fn clone(&self, name: &str) -> PathBuf {
            git(
                &self.root,
                &["clone", "-q", self.origin.to_str().unwrap(), name],
            );
            self.root.join(name)
        }

        /// An executable script `name` under the root, with `body`.
        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.root.join(name);
            write(&self.root, name, &format!("#!/bin/sh\n{body}\n"));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn failures_told_from_git_messages() {
        use Failure::*;
        let conflict = Conflict(Vec::new());
        let samples: &[(&str, &str, Failure, &str)] = &[
            (
                "",
                "Auto-merging a\nCONFLICT (content): Merge conflict in a\n\
                 Automatic merge failed; fix conflicts and then commit the result.\n",
                conflict.clone(),
                "Automatic merge failed; fix conflicts and then commit the result.",
            ),
            (
                "error: could not apply 1a2b3c4... two\nhint: Resolve all conflicts manually, \
                 mark them as resolved with\nhint: \"git add/rm <conflicted_files>\", then run \
                 \"git rebase --continue\".\nCould not apply 1a2b3c4... two\n",
                "",
                conflict.clone(),
                "could not apply 1a2b3c4... two",
            ),
            (
                "a: needs merge\nYou must edit all merge conflicts and then\n\
                 mark them as resolved using git add\n",
                "",
                conflict.clone(),
                "mark them as resolved using git add",
            ),
            (
                "error: Committing is not possible because you have unmerged files.\n\
                 hint: Fix them up in the work tree, and then use 'git add/rm <file>'\n\
                 fatal: Exiting because of an unresolved conflict.\n",
                "",
                conflict.clone(),
                "Exiting because of an unresolved conflict.",
            ),
            (
                "git@github.com: Permission denied (publickey).\r\n\
                 fatal: Could not read from remote repository.\n\n\
                 Please make sure you have the correct access rights\nand the repository exists.\n",
                "",
                Auth,
                "Could not read from remote repository.",
            ),
            (
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled\n",
                "",
                Auth,
                "could not read Username for 'https://github.com': terminal prompts disabled",
            ),
            (
                "remote: Invalid username or password.\n\
                 fatal: Authentication failed for 'https://github.com/o/r.git/'\n",
                "",
                Auth,
                "Authentication failed for 'https://github.com/o/r.git/'",
            ),
            (
                "Host key verification failed.\r\nfatal: Could not read from remote repository.\n",
                "",
                Auth,
                "Could not read from remote repository.",
            ),
            (
                "To /srv/r.git\n ! [rejected]        main -> main (fetch first)\n\
                 error: failed to push some refs to '/srv/r.git'\n\
                 hint: Updates were rejected because the remote contains work that you do not\n",
                "",
                Rejected,
                "failed to push some refs to '/srv/r.git'",
            ),
            (
                " ! [rejected]        feat -> feat (non-fast-forward)\n\
                 error: failed to push some refs to 'github.com:o/r.git'\n",
                "",
                Rejected,
                "failed to push some refs to 'github.com:o/r.git'",
            ),
            (
                " ! [rejected]        feat -> feat (stale info)\n\
                 error: failed to push some refs to '/srv/r.git'\n",
                "",
                Rejected,
                "failed to push some refs to '/srv/r.git'",
            ),
            (
                "hint: Diverging branches can't be fast-forwarded, you need to either:\nhint:\n\
                 hint: \tgit merge --no-ff\nfatal: Not possible to fast-forward, aborting.\n",
                "",
                Diverged,
                "Not possible to fast-forward, aborting.",
            ),
            (
                "fatal: 'x' is already used by worktree at '/tmp/a b/wt'\n",
                "",
                CheckedOut("/tmp/a b/wt".into()),
                "'x' is already used by worktree at '/tmp/a b/wt'",
            ),
            (
                "fatal: 'x' is already checked out at '/tmp/wt'\n",
                "",
                CheckedOut("/tmp/wt".into()),
                "'x' is already checked out at '/tmp/wt'",
            ),
            (
                "fatal: Unable to create '/r/.git/index.lock': File exists.\n\n\
                 Another git process seems to be running in this repository, e.g.\n",
                "",
                Locked,
                "Unable to create '/r/.git/index.lock': File exists.",
            ),
            (
                "error: cannot lock ref 'refs/remotes/origin/main': Unable to create \
                 '/r/.git/refs/remotes/origin/main.lock': File exists.\n",
                "",
                Locked,
                "cannot lock ref 'refs/remotes/origin/main': Unable to create \
                 '/r/.git/refs/remotes/origin/main.lock': File exists.",
            ),
            (
                "error: Unable to create '/r/.git/packed-refs.lock': File exists.\n",
                "",
                Locked,
                "Unable to create '/r/.git/packed-refs.lock': File exists.",
            ),
            (
                "error: could not lock config file .git/config: File exists\n",
                "",
                Locked,
                "could not lock config file .git/config: File exists",
            ),
            (
                "error: cannot lock ref 'refs/remotes/origin/a/b': 'refs/remotes/origin/a' \
                 exists; cannot create 'refs/remotes/origin/a/b'\n",
                "",
                Other,
                "cannot lock ref 'refs/remotes/origin/a/b': 'refs/remotes/origin/a' \
                 exists; cannot create 'refs/remotes/origin/a/b'",
            ),
            (
                "ssh: connect to host 127.0.0.1 port 1: Connection refused\r\n\
                 fatal: Could not read from remote repository.\n",
                "",
                Other,
                "Could not read from remote repository.",
            ),
            (
                "error: pathspec 'nope' did not match any file(s) known to git\n",
                "",
                Other,
                "pathspec 'nope' did not match any file(s) known to git",
            ),
            ("hint: only a hint\n", "", Other, "only a hint"),
            (
                "remote: pushes to lib are frozen until Friday        \n\
                 To /srv/lib.git\n \
                 ! [remote rejected] HEAD -> main (pre-receive hook declined)\n\
                 error: failed to push some refs to '/srv/lib.git'\n",
                "",
                Other,
                "pushes to lib are frozen until Friday (pre-receive hook declined)",
            ),
            (
                "Enumerating objects: 5, done.\n\
                 remote: Resolving deltas: 100% (1/1), completed with 1 local object.\n\
                 remote: error: GH006: Protected branch update failed for refs/heads/main.\n\
                 remote: error: Changes must be made through a pull request.\n\
                 To github.com:o/r.git\n \
                 ! [remote rejected] main -> main (protected branch hook declined)\n\
                 error: failed to push some refs to 'github.com:o/r.git'\n",
                "",
                Other,
                "GH006: Protected branch update failed for refs/heads/main. \
                 Changes must be made through a pull request. (protected branch hook declined)",
            ),
            (
                "To /srv/r.git\n ! [remote rejected] main -> main (shallow update not allowed)\n\
                 error: failed to push some refs to '/srv/r.git'\n",
                "",
                Other,
                "shallow update not allowed",
            ),
        ];
        for (stderr, stdout, want, want_why) in samples {
            assert_eq!(&classify(stderr, stdout), want, "{stderr}{stdout}");
            assert_eq!(why(stderr, stdout), *want_why, "{stderr}{stdout}");
        }
    }

    #[test]
    fn retried_only_where_safe() {
        assert!(retryable(&["fetch", "--all", "--prune"]));
        assert!(retryable(&["push", "-u", "origin", "HEAD"]));
        assert!(retryable(&["--literal-pathspecs", "commit", "-m", "x"]));
        assert!(retryable(&["-c", "a=b", "stash", "push", "-u"]));
        assert!(!retryable(&["stash", "pop"]));
        assert!(!retryable(&["rebase", "origin/main"]));
        assert!(!retryable(&["merge", "--no-edit", "origin/main"]));
        assert!(!retryable(&["pull", "--ff-only"]));
    }

    #[test]
    fn locks_held_tried_again() {
        let fx = Fx::local();
        write(&fx.repo, "a", "changed");
        std::fs::write(fx.repo.join(".git/index.lock"), "").unwrap();
        let failed = commit_paths(&fx.repo, &["a".into()], "m").unwrap_err();
        assert_eq!(failed.failure, Failure::Locked);
        assert!(failed.why.contains("index.lock"), "{}", failed.why);
        // The `add`, tried once and then once per wait.
        assert_eq!(failed.log.len(), 1 + LOCK_RETRIES.len());
        assert!(failed.log.iter().all(|r| !r.ok));
        // Not a stash pop: it may have stopped half-way.
        let failed = stash_pop(&fx.repo, 0, &StashSeen::Count(0)).unwrap_err();
        assert_eq!(failed.log.len(), 1);
        std::fs::remove_file(fx.repo.join(".git/index.lock")).unwrap();
        commit_paths(&fx.repo, &["a".into()], "m").unwrap();
    }

    #[test]
    fn one_turn_per_repository() {
        let (a, b) = (Path::new("/turn/a"), Path::new("/turn/b"));
        let turn = Turn::try_take(a, "a fetch").unwrap();
        assert_eq!(
            Turn::try_take(a, "the push").err().as_deref(),
            Some("a fetch")
        );
        let other = Turn::try_take(b, "the push").unwrap();
        let waiter = std::thread::spawn(move || {
            let _turn = Turn::take(a, "the push");
            Instant::now()
        });
        std::thread::sleep(Duration::from_millis(50));
        let freed = Instant::now();
        drop(turn);
        assert!(waiter.join().unwrap() >= freed);
        drop(other);
        assert!(Turn::try_take(a, "x").is_ok());
        assert!(Turn::try_take(b, "x").is_ok());
    }

    #[test]
    fn commands_as_shell_lines() {
        assert_eq!(
            line(&["push", "-u", "origin", "HEAD"]),
            "git push -u origin HEAD"
        );
        assert_eq!(
            line(&["commit", "-m", "it's done", "--", "a b"]),
            r"git commit -m 'it'\''s done' -- 'a b'"
        );
        let fx = Fx::local();
        let out = exec(&fx.repo, &["status", "--short"], Reach::Local);
        assert_eq!(out.ran.command, "git status --short");
        assert!(out.ran.ok);
    }

    #[test]
    fn heads() {
        let fx = Fx::local();
        let first = rev(&fx.repo, "HEAD");
        let on_main = Head {
            branch: Some("main".into()),
            oid: Some(first.clone()),
        };
        assert_eq!(head(&fx.repo), Some(on_main));
        git(&fx.repo, &["switch", "-q", "--detach"]);
        let detached = Head {
            branch: None,
            oid: Some(first),
        };
        assert_eq!(head(&fx.repo), Some(detached));
        let unborn = fx.root.join("unborn");
        git(&fx.root, &["init", "-q", "-b", "trunk", "unborn"]);
        let new = Head {
            branch: Some("trunk".into()),
            oid: None,
        };
        assert_eq!(head(&unborn), Some(new));
        let plain = fx.root.join("plain");
        std::fs::create_dir(&plain).unwrap();
        // Unless the temporary folder is in a repository itself.
        if query(&plain, &["rev-parse", "--git-dir"]).is_none() {
            assert_eq!(head(&plain), None);
        }
    }

    #[test]
    fn switch_to_local_remote_and_new_branches() {
        let fx = Fx::new();
        git(&fx.repo, &["branch", "loc"]);
        let log = switch_or_create(&fx.repo, "loc").unwrap();
        assert_eq!(log[0].command, "git switch loc");
        assert_eq!(branch(&fx.repo).as_deref(), Some("loc"));

        // Only the remote has it: a branch tracking the remote's.
        let other = fx.clone("other");
        git(&other, &["switch", "-q", "-c", "rem/x"]);
        commit(&other, "r", "remote work");
        git(&other, &["push", "-q", "origin", "rem/x"]);
        git(&fx.repo, &["fetch", "-q"]);
        let log = switch_or_create(&fx.repo, "rem/x").unwrap();
        assert_eq!(log[0].command, "git switch -c rem/x --track origin/rem/x");
        assert_eq!(rev(&fx.repo, "HEAD"), rev(&other, "HEAD"));
        let up = git(&fx.repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert_eq!(up.trim(), "origin/rem/x");

        // Neither: a new one from HEAD, no upstream.
        switch_or_create(&fx.repo, "brand/new").unwrap();
        assert_eq!(branch(&fx.repo).as_deref(), Some("brand/new"));
        assert_eq!(rev(&fx.repo, "HEAD"), rev(&other, "HEAD"));
        assert!(query(&fx.repo, &["rev-parse", "-q", "--verify", "@{upstream}"]).is_none());
        assert_eq!(
            failure(switch_or_create(&fx.repo, "bad..name")),
            Failure::Other
        );
    }

    #[test]
    fn the_first_name_a_repository_has_a_branch_by() {
        let fx = Fx::new();
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let found = |n: &[&str]| branch_named(&fx.repo, &names(n), Some("origin"));
        assert_eq!(found(&["main", "master"]).as_deref(), Some("main"));
        assert_eq!(found(&["master", "main"]).as_deref(), Some("main"));
        assert_eq!(found(&["trunk"]), None);
        // A local one first, though later in the group.
        git(&fx.repo, &["push", "-q", "origin", "main:master"]);
        git(&fx.repo, &["fetch", "-q"]);
        assert_eq!(found(&["master", "main"]).as_deref(), Some("main"));
        git(&fx.repo, &["branch", "-q", "-m", "main", "trunk"]);
        assert_eq!(found(&["master", "main"]).as_deref(), Some("master"));
        assert_eq!(branch_named(&fx.repo, &names(&["master"]), None), None);
    }

    #[test]
    fn a_merge_from_takes_the_newer_of_a_branch_and_its_remote() {
        let fx = Fx::new();
        let dir = &fx.repo;
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let source = |n: &[&str]| source_of(dir, &names(n), Some("origin"));
        // The same commit: the local one.
        assert_eq!(source(&["master", "main"]).as_deref(), Some("main"));
        assert_eq!(source(&["dev"]), None);
        // Only on the remote.
        git(dir, &["push", "-q", "origin", "main:dev"]);
        git(dir, &["fetch", "-q"]);
        assert_eq!(source(&["dev"]).as_deref(), Some("origin/dev"));
        assert_eq!(source_of(dir, &names(&["dev"]), None), None);
        // Fetched, not pulled: the remote's.
        let other = fx.clone("other");
        commit(&other, "o", "theirs");
        git(&other, &["push", "-q"]);
        git(dir, &["fetch", "-q"]);
        assert_eq!(source(&["main"]).as_deref(), Some("origin/main"));
        // Not pushed yet: the local one.
        git(dir, &["merge", "-q", "--ff-only", "origin/main"]);
        commit(dir, "m", "mine");
        assert_eq!(source(&["main"]).as_deref(), Some("main"));
    }

    #[test]
    fn a_branch_checked_out_elsewhere() {
        let fx = Fx::new();
        let wt = fx.root.join("wt");
        git(
            &fx.repo,
            &["worktree", "add", "-q", "-b", "wtb", wt.to_str().unwrap()],
        );
        assert_eq!(
            failure(switch_or_create(&fx.repo, "wtb")),
            Failure::CheckedOut(wt.clone())
        );
    }

    #[test]
    fn a_branch_is_deleted_only_when_nothing_is_lost() {
        let fx = Fx::new();
        let r = &fx.repo;
        let commit_on = |branch: &str| {
            git(r, &["switch", "-q", "-c", branch]);
            git(r, &["commit", "-q", "--allow-empty", "-m", branch]);
            git(r, &["switch", "-q", "main"]);
        };
        git(r, &["branch", "-q", "merged"]);
        commit_on("feat");
        assert_eq!(unmerged_commits(r, "merged"), Some(0));
        assert_eq!(unmerged_commits(r, "feat"), Some(1));
        assert_eq!(
            failure(delete_branch(r, "feat", false)),
            Failure::Blocked("1 commit in no other branch".into())
        );
        assert!(has_ref(r, "refs/heads/feat"));
        // Its own remote copy isn't a home: a pruning fetch can drop it.
        git(r, &["push", "-q", "origin", "feat"]);
        assert_eq!(unmerged_commits(r, "feat"), Some(1));
        // Another remote branch is.
        git(r, &["push", "-q", "origin", "feat:kept"]);
        assert_eq!(unmerged_commits(r, "feat"), Some(0));
        for name in ["merged", "feat"] {
            assert!(delete_branch(r, name, false).is_ok());
            assert!(!has_ref(r, &format!("refs/heads/{name}")));
        }
        // Forced, whatever it loses.
        commit_on("lost");
        assert!(delete_branch(r, "lost", true).is_ok());
        assert!(!has_ref(r, "refs/heads/lost"));
        // Not while another worktree has it out.
        let wt = fx.root.join("wt");
        git(
            r,
            &["worktree", "add", "-q", "-b", "wtb", wt.to_str().unwrap()],
        );
        assert_eq!(
            failure(delete_branch(r, "wtb", true)),
            Failure::CheckedOut(wt)
        );
    }

    #[test]
    fn a_squashed_branch_loses_nothing() {
        let fx = Fx::new();
        let r = &fx.repo;
        // Branch `name` off main, committing each of `texts` to a file of its name.
        let branch = |name: &str, texts: &[&str]| {
            git(r, &["switch", "-q", "-c", name, "main"]);
            for text in texts {
                commit(r, name, text);
            }
        };
        let squash = |name: &str, into: &str| {
            git(r, &["switch", "-q", into]);
            git(r, &["merge", "-q", "--squash", name]);
            git(r, &["commit", "-q", "-m", name]);
        };
        branch("squashed", &["one", "two"]);
        squash("squashed", "main");
        assert_eq!(unmerged_commits(r, "squashed"), Some(0));
        assert!(delete_branch(r, "squashed", false).is_ok());
        // Not when only some of it is in.
        branch("partly", &["one"]);
        squash("partly", "main");
        git(r, &["switch", "-q", "partly"]);
        commit(r, "partly-more", "two");
        assert_eq!(unmerged_commits(r, "partly"), Some(2));
        // Not when it's been undone since.
        branch("reverted", &["one"]);
        squash("reverted", "main");
        git(r, &["revert", "--no-edit", "HEAD"]);
        assert_eq!(unmerged_commits(r, "reverted"), Some(1));
        // Still when changed again since, though that merge conflicts.
        branch("changed", &["one"]);
        squash("changed", "main");
        commit(r, "changed", "other");
        assert_eq!(unmerged_commits(r, "changed"), Some(0));
        // A branch that changes nothing loses its commits.
        branch("undone", &["one"]);
        git(r, &["rm", "-q", "undone"]);
        git(r, &["commit", "-q", "-m", "undo"]);
        assert_eq!(unmerged_commits(r, "undone"), Some(2));
        // Squashed into the remote's default branch, not main.
        git(r, &["switch", "-q", "-c", "dev", "main"]);
        branch("on-dev", &["one"]);
        squash("on-dev", "dev");
        git(r, &["push", "-q", "origin", "dev"]);
        git(r, &["remote", "set-head", "origin", "dev"]);
        git(r, &["switch", "-q", "main"]);
        git(r, &["branch", "-q", "-D", "dev"]);
        assert_eq!(unmerged_commits(r, "on-dev"), Some(0));
        // Not merely unmerged.
        branch("feat", &["one"]);
        git(r, &["switch", "-q", "main"]);
        assert_eq!(unmerged_commits(r, "feat"), Some(1));
    }

    #[test]
    fn remote_default_sees_past_a_stale_origin_head() {
        let t = std::env::temp_dir().join(format!("gerdr-remote-default-{}", std::process::id()));
        let (bare, r) = (t.join("o.git"), t.join("r"));
        std::fs::create_dir_all(&r).unwrap();
        git(&t, &["init", "-q", "--bare", "-b", "main", "o.git"]);
        git(&r, &["init", "-q", "-b", "main"]);
        assert_eq!(remote_default(&r), Ok(None));
        git(&r, &["remote", "add", "origin", &bare.to_string_lossy()]);
        git(&r, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&r, &["push", "-q", "origin", "main", "main:dev"]);
        git(&r, &["remote", "set-head", "origin", "main"]);
        assert_eq!(remote_default(&r), Ok(Some("main".to_owned())));
        // The remote's default moves; the clone's pointer doesn't.
        git(&bare, &["symbolic-ref", "HEAD", "refs/heads/dev"]);
        git(&r, &["fetch", "-q", "origin"]);
        assert_eq!(remote_default(&r), Ok(Some("dev".to_owned())));
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// The files HEAD's commit changed, as `<letter> <path>`.
    fn committed(dir: &Path) -> Vec<String> {
        let out = git(
            dir,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-status",
                "-r",
                "-z",
                "--no-renames",
                "HEAD",
            ],
        );
        let fields: Vec<&str> = out.split('\0').filter(|f| !f.is_empty()).collect();
        let mut all: Vec<String> = fields
            .chunks(2)
            .map(|c| format!("{} {}", c[0], c[1]))
            .collect();
        all.sort();
        all
    }

    #[test]
    fn commit_only_the_picked_files() {
        let fx = Fx::local();
        let dir = &fx.repo;
        for file in [
            "mod.txt",
            "gone.txt",
            "old.txt",
            "staged.txt",
            "sp ace ü.txt",
        ] {
            commit(dir, file, file);
        }
        write(dir, "mod.txt", "changed");
        write(dir, "new.txt", "new");
        write(dir, "d/x", "x");
        write(dir, "d/y/z", "z");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        git(dir, &["mv", "old.txt", "renamed.txt"]);
        write(dir, "sp ace ü.txt", "changed");
        // Not picked: a staged file, a changed one a glob would catch, and
        // an untracked one.
        write(dir, "staged.txt", "staged");
        git(dir, &["add", "staged.txt"]);
        write(dir, "a", "changed");
        write(dir, "a*", "star");
        write(dir, "keep.txt", "keep");
        let paths: Vec<String> = [
            "mod.txt",
            "new.txt",
            "d/",
            "gone.txt",
            "old.txt",
            "renamed.txt",
            "sp ace ü.txt",
            "a*",
        ]
        .map(String::from)
        .into();
        let log = commit_paths(dir, &paths, "picked").unwrap();
        assert_eq!(log.len(), 2);
        assert!(
            log[1]
                .command
                .starts_with("git --literal-pathspecs commit -m picked --only")
        );
        assert_eq!(
            committed(dir),
            [
                "A a*",
                "A d/x",
                "A d/y/z",
                "A new.txt",
                "A renamed.txt",
                "D gone.txt",
                "D old.txt",
                "M mod.txt",
                "M sp ace ü.txt",
            ]
        );
        assert_eq!(git(dir, &["log", "-1", "--format=%s"]).trim(), "picked");
        // What wasn't picked is as it was: staged, changed, untracked.
        let status = git(dir, &["status", "--porcelain", "-z"]);
        let mut status: Vec<&str> = status.split('\0').filter(|s| !s.is_empty()).collect();
        status.sort();
        assert_eq!(status, [" M a", "?? keep.txt", "M  staged.txt"]);
        // The staged one is still the staged text.
        assert_eq!(git(dir, &["show", ":staged.txt"]), "staged");

        assert_eq!(
            commit_paths(dir, &[], "x").unwrap_err().failure,
            Failure::Blocked("nothing checked".into())
        );
        // From a folder inside, paths are still from the top.
        write(dir, "d/x", "again");
        commit_paths(&dir.join("d"), &["d/x".into()], "inside").unwrap();
        assert_eq!(committed(dir), ["M d/x"]);
    }

    #[test]
    fn amend_takes_the_paths_and_the_message_into_the_last_commit() {
        let fx = Fx::local();
        let dir = &fx.repo;
        commit(dir, "a", "one");
        let parent = rev(dir, "HEAD~1");
        write(dir, "a", "two");
        write(dir, "b", "new");
        write(dir, "staged.txt", "staged");
        git(dir, &["add", "staged.txt"]);
        // The file checked comes in, the message stays without one.
        let log = amend_paths(dir, &["b".into()], None).unwrap();
        assert!(
            log.last()
                .unwrap()
                .command
                .contains("commit --amend --no-edit --only -- b")
        );
        assert_eq!(rev(dir, "HEAD~1"), parent);
        assert_eq!(git(dir, &["log", "-1", "--format=%s"]).trim(), "one");
        assert_eq!(committed(dir), ["A b", "M a"]);
        // What wasn't checked is as it was.
        assert_eq!(git(dir, &["show", ":staged.txt"]), "staged");
        assert_eq!(read(dir, "a"), "two");
        // Without paths it only rewords, what is staged left out.
        amend_paths(dir, &[], Some("reworded")).unwrap();
        assert_eq!(git(dir, &["log", "-1", "--format=%s"]).trim(), "reworded");
        assert_eq!(committed(dir), ["A b", "M a"]);
        assert_eq!(rev(dir, "HEAD~1"), parent);
    }

    #[test]
    fn amend_before_the_first_commit_is_blocked() {
        let fx = Fx::local();
        git(&fx.root, &["init", "-q", "-b", "main", "unborn"]);
        let dir = fx.root.join("unborn");
        write(&dir, "one", "1");
        assert_eq!(
            failure(amend_paths(&dir, &["one".into()], Some("x"))),
            Failure::Blocked("no commit to amend".into())
        );
    }

    #[test]
    fn commit_before_the_first_commit() {
        let fx = Fx::local();
        git(&fx.root, &["init", "-q", "-b", "main", "unborn"]);
        let dir = fx.root.join("unborn");
        write(&dir, "one", "1");
        write(&dir, "two", "2");
        commit_paths(&dir, &["one".into()], "first").unwrap();
        assert_eq!(committed(&dir), ["A one"]);
        let status = git(&dir, &["status", "--porcelain"]);
        assert_eq!(status, "?? two\n");
    }

    #[test]
    fn pushes() {
        let fx = Fx::new();
        let dir = &fx.repo;
        switch_or_create(dir, "feat").unwrap();
        commit(dir, "f", "one");
        let log = push(dir, None, false).unwrap();
        assert_eq!(log[0].command, "git push -u origin HEAD");
        let up = git(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert_eq!(up.trim(), "origin/feat");
        commit(dir, "f", "two");
        // Named in full: no push.default of the user's in the way.
        assert_eq!(
            push(dir, None, false).unwrap()[0].command,
            "git push origin HEAD:refs/heads/feat"
        );

        // Rewritten: refused, then pushed with a lease.
        git(dir, &["commit", "-q", "--amend", "-m", "two again"]);
        assert_eq!(failure(push(dir, None, false)), Failure::Rejected);
        let log = push(dir, None, true).unwrap();
        assert_eq!(
            log[0].command,
            "git push --force-with-lease --force-if-includes origin HEAD:refs/heads/feat"
        );
        assert_eq!(rev(&fx.origin, "feat"), rev(dir, "HEAD"));

        // Someone else pushed: refused, with a lease too.
        let other = fx.clone("other");
        git(&other, &["switch", "-q", "feat"]);
        commit(&other, "o", "theirs");
        git(&other, &["push", "-q"]);
        commit(dir, "f", "three");
        let failed = push(dir, None, false).unwrap_err();
        assert_eq!(failed.failure, Failure::Rejected);
        assert!(
            failed.why.starts_with("failed to push some refs"),
            "{}",
            failed.why
        );
        git(dir, &["fetch", "-q"]);
        commit(dir, "f", "four");
        assert_eq!(failure(push(dir, None, false)), Failure::Rejected);
        // Fetched, their commit is what the lease expects: only that it
        // isn't in this branch keeps it from being pushed over.
        assert_eq!(failure(push(dir, None, true)), Failure::Rejected);
        assert_eq!(rev(&fx.origin, "feat"), rev(&other, "HEAD"));

        // An upstream of another name: a branch of its own.
        git(
            dir,
            &["switch", "-q", "-c", "side", "--track", "origin/main"],
        );
        let log = push(dir, None, false).unwrap();
        assert_eq!(log[0].command, "git push -u origin HEAD");
        let up = git(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert_eq!(up.trim(), "origin/side");

        git(dir, &["switch", "-q", "--detach"]);
        assert_eq!(
            failure(push(dir, None, false)),
            Failure::Blocked("detached HEAD".into())
        );
        let local = Fx::local();
        assert_eq!(
            failure(push(&local.repo, None, false)),
            Failure::Blocked("no remote".into())
        );
    }

    #[test]
    fn pulls() {
        let fx = Fx::new();
        let dir = &fx.repo;
        let other = fx.clone("other");
        commit(&other, "o", "theirs 1");
        git(&other, &["push", "-q"]);
        git(dir, &["fetch", "-q"]);
        let log = pull(dir, Pull::FastForward).unwrap();
        assert_eq!(log[0].command, "git pull --no-rebase --ff-only");
        assert_eq!(rev(dir, "HEAD"), rev(&other, "HEAD"));

        // Both moved on.
        commit(&other, "o", "theirs 2");
        git(&other, &["push", "-q"]);
        commit(dir, "m", "mine 1");
        let mine = rev(dir, "HEAD");
        assert_eq!(failure(pull(dir, Pull::FastForward)), Failure::Diverged);
        assert_eq!(rev(dir, "HEAD"), mine);
        pull(dir, Pull::Rebase).unwrap();
        assert_eq!(rev(dir, "HEAD~1"), rev(&other, "HEAD"));
        assert_eq!(read(dir, "m"), "mine 1");

        // A merge, beside an uncommitted change it doesn't touch.
        commit(&other, "o", "theirs 3");
        git(&other, &["push", "-q"]);
        commit(dir, "m", "mine 2");
        write(dir, "a", "uncommitted");
        pull(dir, Pull::Merge).unwrap();
        assert_eq!(rev(dir, "HEAD^2"), rev(&other, "HEAD"));
        assert_eq!(read(dir, "a"), "uncommitted");

        switch_or_create(dir, "lonely").unwrap();
        assert_eq!(
            failure(pull(dir, Pull::FastForward)),
            Failure::Blocked("no upstream".into())
        );
    }

    /// Branch feat off main, both changing `a`.
    fn diverged_on_a(dir: &Path) {
        git(dir, &["switch", "-q", "-c", "feat"]);
        commit(dir, "a", "feat's a");
        git(dir, &["switch", "-q", "main"]);
        commit(dir, "a", "main's a");
        commit(dir, "b c", "main's b c");
        git(dir, &["switch", "-q", "feat"]);
    }

    #[test]
    fn a_merge_conflict_continued_and_aborted() {
        let fx = Fx::local();
        let dir = &fx.repo;
        diverged_on_a(dir);
        let tip = rev(dir, "HEAD");
        assert_eq!(
            predict_merge(dir, "HEAD", "main"),
            Verdict::Conflicts(vec!["a".into()])
        );
        let failed = integrate(dir, "main", Integrate::Merge).unwrap_err();
        assert_eq!(failed.failure, Failure::Conflict(vec!["a".into()]));
        assert_eq!(
            failed.log[0].command,
            "git merge --no-edit --no-autostash main"
        );
        // Not resolved yet: still conflicted.
        assert_eq!(
            failure(continue_op(dir, InProgress::Merge)),
            Failure::Conflict(vec!["a".into()])
        );
        abort_op(dir, InProgress::Merge).unwrap();
        assert_eq!(rev(dir, "HEAD"), tip);
        assert_eq!(read(dir, "a"), "feat's a");
        assert!(git(dir, &["status", "--porcelain"]).is_empty());

        integrate(dir, "main", Integrate::Merge).unwrap_err();
        write(dir, "a", "both");
        mark_resolved(dir, "a").unwrap();
        continue_op(dir, InProgress::Merge).unwrap();
        assert_eq!(rev(dir, "HEAD^1"), tip);
        assert_eq!(rev(dir, "HEAD^2"), rev(dir, "main"));
        assert_eq!(
            git(dir, &["log", "-1", "--format=%s"]).trim(),
            "Merge branch 'main' into feat"
        );
        assert!(git(dir, &["status", "--porcelain"]).is_empty());
    }

    #[test]
    fn a_rebase_stops_on_each_conflicting_commit() {
        let fx = Fx::local();
        let dir = &fx.repo;
        git(dir, &["switch", "-q", "-c", "feat"]);
        commit(dir, "a", "f1");
        commit(dir, "a", "f2");
        git(dir, &["switch", "-q", "main"]);
        commit(dir, "a", "m1");
        git(dir, &["switch", "-q", "feat"]);
        let tip = rev(dir, "HEAD");
        let rebase = InProgress::Rebase { step: 1, total: 2 };

        assert_eq!(
            failure(integrate(dir, "main", Integrate::Rebase)),
            Failure::Conflict(vec!["a".into()])
        );
        abort_op(dir, rebase).unwrap();
        assert_eq!(rev(dir, "HEAD"), tip);
        assert_eq!(branch(dir).as_deref(), Some("feat"));

        integrate(dir, "main", Integrate::Rebase).unwrap_err();
        write(dir, "a", "r1");
        mark_resolved(dir, "a").unwrap();
        assert_eq!(
            failure(continue_op(dir, rebase)),
            Failure::Conflict(vec!["a".into()])
        );
        write(dir, "a", "r2");
        mark_resolved(dir, "a").unwrap();
        continue_op(dir, rebase).unwrap();
        assert_eq!(branch(dir).as_deref(), Some("feat"));
        assert_eq!(rev(dir, "HEAD~2"), rev(dir, "main"));
        assert_eq!(read(dir, "a"), "r2");
        let subjects = git(dir, &["log", "-2", "--format=%s"]);
        assert_eq!(subjects, "f2\nf1\n");
    }

    #[test]
    fn undo_a_batch_unless_head_moved() {
        let fx = Fx::local();
        let dir = &fx.repo;
        let before = head(dir).unwrap();
        // A branch switch is undone by switching back, and only that: main
        // moving on meanwhile (in another worktree, say) stays.
        switch_or_create(dir, "feat").unwrap();
        commit(dir, "f", "feat");
        let after = head(dir).unwrap();
        git(dir, &["branch", "-f", "main", "feat"]);
        undo(dir, &before, &after, false).unwrap();
        assert_eq!(branch(dir).as_deref(), Some("main"));
        assert_eq!(head_oid(dir), after.oid);
        git(
            dir,
            &["reset", "-q", "--keep", before.oid.as_deref().unwrap()],
        );

        // A commit undone keeps its changes, staged; what wasn't committed
        // stays as it was.
        commit(dir, "b", "batch");
        let after = head(dir).unwrap();
        write(dir, "a", "uncommitted");
        commit(dir, "c", "since");
        let failed = undo(dir, &before, &after, true).unwrap_err();
        assert_eq!(failed.failure, Failure::Blocked("HEAD moved since".into()));
        assert!(failed.log.is_empty());
        git(dir, &["reset", "-q", "--keep", "HEAD~1"]);
        undo(dir, &before, &after, true).unwrap();
        assert_eq!(head(dir), Some(before.clone()));
        assert_eq!(read(dir, "b"), "batch");
        assert_eq!(read(dir, "a"), "uncommitted");
        assert_eq!(git(dir, &["status", "--porcelain"]), " M a\nA  b\n");

        // A merge or a pull undone goes, what wasn't committed stays.
        git(dir, &["reset", "-q", "HEAD", "--", "b"]);
        std::fs::remove_file(dir.join("b")).unwrap();
        commit(dir, "d", "merged");
        let after = head(dir).unwrap();
        undo(dir, &before, &after, false).unwrap();
        assert_eq!(head(dir), Some(before.clone()));
        assert!(!dir.join("d").exists());
        assert_eq!(read(dir, "a"), "uncommitted");

        let unborn = Head {
            branch: Some("main".into()),
            oid: None,
        };
        let now = head(dir).unwrap();
        assert_eq!(
            failure(undo(dir, &unborn, &now, false)),
            Failure::Blocked("no commit to go back to".into())
        );
    }

    #[test]
    fn merge_predictions() {
        let fx = Fx::local();
        let dir = &fx.repo;
        git(dir, &["switch", "-q", "-c", "feat"]);
        commit(dir, "f", "feat");
        commit(dir, "sp ace", "feat's");
        git(dir, &["switch", "-q", "main"]);
        commit(dir, "m", "main");
        assert_eq!(predict_merge(dir, "feat", "main"), Verdict::Clean);
        commit(dir, "sp ace", "main's");
        commit(dir, "f", "main's f");
        assert_eq!(
            predict_merge(dir, "feat", "main"),
            Verdict::Conflicts(vec!["f".into(), "sp ace".into()])
        );
        assert!(matches!(
            predict_merge(dir, "feat", "nope"),
            Verdict::Blocked(why) if why.contains("nope")
        ));
        // Nothing was touched.
        assert_eq!(branch(dir).as_deref(), Some("main"));
        assert!(git(dir, &["status", "--porcelain"]).is_empty());
    }

    #[test]
    fn what_blocks_a_switch() {
        let fx = Fx::new();
        let dir = &fx.repo;
        commit(dir, "b", "b");
        git(dir, &["push", "-q"]);
        git(dir, &["switch", "-q", "-c", "other"]);
        commit(dir, "a", "other's a");
        commit(dir, "d/x", "other's d/x");
        git(dir, &["switch", "-q", "main"]);
        let remote = fx.clone("remote");
        git(&remote, &["switch", "-q", "-c", "far"]);
        commit(&remote, "b", "far's b");
        git(&remote, &["push", "-q", "origin", "far"]);
        git(dir, &["fetch", "-q"]);

        write(dir, "a", "dirty");
        write(dir, "b", "dirty");
        write(dir, "d/x", "untracked");
        let dirty: Vec<String> = ["a", "b", "d/"].map(String::from).into();
        assert_eq!(switch_blockers(dir, "other", &dirty), ["a", "d/"]);
        assert_eq!(switch_blockers(dir, "far", &dirty), ["b"]);
        assert_eq!(switch_blockers(dir, "origin/far", &dirty), ["b"]);
        assert!(switch_blockers(dir, "main", &dirty).is_empty());
        assert!(switch_blockers(dir, "nowhere", &dirty).is_empty());
        assert!(switch_blockers(dir, "other", &[]).is_empty());
    }

    #[test]
    fn diffs_for_the_detail_pane() {
        let fx = Fx::local();
        let dir = &fx.repo;
        let change = |path: &str, orig: Option<&str>, code| Change {
            path: path.into(),
            orig: orig.map(String::from),
            code,
        };
        let lines = |f: &FileDiff| -> Vec<String> {
            let kind = |k| match k {
                LineKind::Added => '+',
                LineKind::Removed => '-',
                LineKind::Context => ' ',
            };
            f.hunks
                .iter()
                .flat_map(|h| &h.lines)
                .map(|l| format!("{}{}", kind(l.kind), l.text))
                .collect()
        };
        write(dir, "a", "changed\n");
        write(dir, "new file", "hello\n");
        write(dir, "u/one", "1");
        write(dir, "u/two/three", "3");
        write(dir, "bin", "\0\0");
        commit(dir, "old", "moved\n");
        git(dir, &["mv", "old", "moved"]);
        let changes = [
            change("a", None, 'M'),
            change("new file", None, '?'),
            change("u/", None, '?'),
            change("moved", Some("old"), 'R'),
            change("bin", None, '?'),
            change("gone", None, 'D'),
        ];
        let d = repo_diff(dir, &changes);
        let paths: Vec<&str> = d.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a", "new file", "u/", "moved", "bin", "gone"]);
        assert_eq!(lines(&d[0]), ["-first", "+changed"]);
        assert_eq!((d[0].added, d[0].removed, d[0].code), (1, 1, 'M'));
        assert_eq!(lines(&d[1]), ["+hello"]);
        assert_eq!(d[1].code, '?');
        assert_eq!(d[2].notes, ["u/one", "u/two/three"]);
        assert_eq!(d[3].orig.as_deref(), Some("old"));
        assert!(d[3].hunks.is_empty(), "{:?}", d[3]);
        assert_eq!(d[4].notes, ["Binary file"]);
        // Not changed after all: said so.
        assert_eq!(d[5].notes, ["No changes to show"]);

        let big: String = (0..6000).map(|n| format!("{n}\n")).collect();
        write(dir, "big", &big);
        let d = repo_diff(dir, &[change("big", None, '?')]);
        assert_eq!(d[0].added, 6000);
        assert_eq!(d[0].hunks[0].lines.len(), 6000);

        // A new file's code is coloured as its own language.
        write(dir, "new.rs", "fn main() {}\n");
        let d = repo_diff(dir, &[change("new.rs", None, '?')]);
        assert!(!d[0].hunks[0].lines[0].syntax.is_empty(), "{:?}", d[0]);

        git(&fx.root, &["init", "-q", "-b", "main", "unborn"]);
        let unborn = fx.root.join("unborn");
        write(&unborn, "x", "x\n");
        git(&unborn, &["add", "x"]);
        let d = repo_diff(&unborn, &[change("x", None, 'A')]);
        assert_eq!(lines(&d[0]), ["+x"]);
    }

    #[test]
    fn a_restore_puts_a_file_back_as_git_had_it() {
        let fx = Fx::local();
        let dir = &fx.repo;
        let cases = [
            (
                "tabs.go",
                "func f() {\n\tg()\n\th()\n}\n",
                "func f() {\n\tif x == nil {\n\t\treturn\n\t}\n\tg()\n\tk()\n}\n",
            ),
            ("crlf.txt", "a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\nd\r\n"),
            ("open.txt", "a\nb", "z\na\nb\nc"),
        ];
        for (path, before, after) in cases {
            write(dir, path, before);
            git(dir, &["add", "--", path]);
            git(dir, &["commit", "-q", "-m", path]);
            write(dir, path, after);
            let change = Change {
                path: path.into(),
                orig: None,
                code: 'M',
            };
            let file = repo_diff(dir, &[change]).pop().unwrap();
            let (cut, _) = crate::diff_edit::restore_cut(&file, |_, _| true).unwrap();
            let edit = crate::diff_edit::FileEdit {
                root: dir.clone(),
                path: path.into(),
                cut,
                shown: crate::diff_edit::shown(&file),
                caret: Default::default(),
            };
            crate::diff_edit::apply(&edit).unwrap();
            assert_eq!(read(dir, path), before, "{path}");
        }
    }

    #[test]
    fn a_worktree_changed_since_it_forked() {
        let fx = Fx::local();
        let main = &fx.repo;
        let wt = fx.root.join("wt");
        let wt_path = wt.to_str().unwrap();
        git(main, &["worktree", "add", "-q", "-b", "feat", wt_path]);
        // Main moves on: none of it is the worktree's.
        commit(main, "main-only", "on main\n");
        commit(&wt, "a", "changed\n");
        commit(&wt, "old", "moved\n");
        git(&wt, &["mv", "old", "moved"]);
        git(&wt, &["commit", "-q", "-m", "move"]);
        write(&wt, "b", "not committed\n");
        git(&wt, &["add", "b"]);
        write(&wt, "new", "untracked\n");
        write(&wt, "dir/x", "x\n");
        let since = changes_since(&wt, Some("main")).unwrap();
        assert_eq!(since.base, rev(main, "main~1"));
        assert_eq!(since.commits, 3);
        let changes: Vec<(&str, Option<&str>, char)> = since
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.orig.as_deref(), c.code))
            .collect();
        assert_eq!(
            changes,
            [
                ("a", None, 'M'),
                ("b", None, 'A'),
                ("moved", None, 'A'),
                ("dir/", None, '?'),
                ("new", None, '?'),
            ]
        );
        // Its diff is against where it forked, committed or not.
        let d = repo_diff_from(&wt, &since.changes, Some(&since.base));
        let paths: Vec<&str> = d.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a", "b", "moved", "dir/", "new"]);
        assert_eq!((d[0].added, d[0].removed), (1, 1));
        assert_eq!(d[3].notes, ["dir/x"]);
        // A main worktree's: what isn't committed.
        write(main, "a", "dirty\n");
        let since = changes_since(main, None).unwrap();
        assert_eq!((since.base, since.commits), (rev(main, "HEAD"), 0));
        assert_eq!(since.changes.len(), 1);
        // Nothing in common, it says so.
        assert!(changes_since(&wt, Some("no-such-branch")).is_err());
    }

    /// The lines of `log`, without the commits' ids.
    fn log_text(log: &CommitLog) -> Vec<String> {
        log.lines().into_iter().map(|(l, _)| l).collect()
    }

    #[test]
    fn logs_commits_to_push_and_pull() {
        let fx = Fx::new();
        let dir = &fx.repo;
        let log = log_upstream(dir);
        let text = log_text(&log);
        assert_eq!(text[0], "Up to date with origin/main. The last commits:");
        assert_eq!(text.len(), 2);
        assert!(text[1].contains(" first · "), "{text:?}");
        assert_eq!(log.lines()[1].1, Some(rev(dir, "HEAD")));
        // Someone else pushes two; this one commits one.
        let other = fx.clone("other");
        commit(&other, "x", "theirs one");
        commit(&other, "y", "theirs two");
        git(&other, &["push", "-q"]);
        git(dir, &["fetch", "-q"]);
        commit(dir, "b", "mine");
        let log = log_upstream(dir);
        let text = log_text(&log);
        assert_eq!(text[0], "To pull from origin/main (2):");
        assert!(
            text[1].starts_with(&rev(dir, "origin/main")[..7]),
            "{text:?}"
        );
        assert!(text[1].contains(" theirs two · "), "{text:?}");
        assert!(text[2].contains(" theirs one · "), "{text:?}");
        assert_eq!(text[3], "");
        assert_eq!(text[4], "To push to origin/main (1):");
        assert!(text[5].contains(" mine · "), "{text:?}");
        let oids: Vec<_> = log.lines().into_iter().map(|(_, o)| o).collect();
        assert_eq!(oids[0], None);
        assert_eq!(oids[1], Some(rev(dir, "origin/main")));
        assert_eq!(oids[3], None);
        assert_eq!(oids[5], Some(rev(dir, "HEAD")));
        // Only to push: the other list says there is none.
        git(dir, &["pull", "-q", "--rebase"]);
        let text = log_text(&log_upstream(dir));
        assert_eq!(text[0], "To pull from origin/main: none");
        assert_eq!(text[2], "To push to origin/main (1):");
        assert_eq!(
            commits_between(dir, "@{u}..", 5),
            [(rev(dir, "HEAD")[..7].to_owned(), "mine".to_owned())]
        );
        switch_or_create(dir, "lonely").unwrap();
        let text = log_text(&log_upstream(dir));
        assert_eq!(text[0], "No upstream yet. The last commits:");
        assert_eq!(text.len(), 5);
    }

    #[test]
    fn reads_a_commits_diff() {
        let fx = Fx::local();
        let dir = &fx.repo;
        // The first commit: all added.
        let (changes, files) = commit_diff(dir, &rev(dir, "HEAD")).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!((changes[0].path.as_str(), changes[0].code), ("a", 'A'));
        assert_eq!(files[0].added, 1);
        // A change, a rename and an add, then a later commit that isn't in it.
        write(dir, "big", &"line\n".repeat(20));
        commit(dir, "a", "changed");
        git(dir, &["add", "big"]);
        git(dir, &["commit", "-q", "-m", "big"]);
        git(dir, &["mv", "big", "moved"]);
        write(dir, "c", "new\n");
        git(dir, &["add", "c"]);
        commit(dir, "a", "again");
        commit(dir, "c", "later");
        let (changes, files) = commit_diff(dir, &rev(dir, "HEAD~1")).unwrap();
        let paths: Vec<_> = changes.iter().map(|c| (c.path.as_str(), c.code)).collect();
        assert_eq!(paths, [("a", 'M'), ("c", 'A'), ("moved", 'R')]);
        assert_eq!(changes[2].orig.as_deref(), Some("big"));
        assert_eq!((files[0].added, files[0].removed), (1, 1));
        assert_eq!(files[1].added, 1);
        assert_eq!(files[2].path, "moved");
        assert!(commit_diff(dir, "no-such-commit").is_err());
    }

    #[test]
    fn reads_what_commits_from_one_to_another_changed() {
        let fx = Fx::local();
        let dir = &fx.repo;
        let from = rev(dir, "HEAD");
        commit(dir, "a", "changed");
        commit(dir, "b", "added");
        let to = rev(dir, "HEAD");
        // Uncommitted and later work isn't the range's.
        commit(dir, "c", "later");
        write(dir, "a", "dirty\n");
        let (changes, files) = range_diff(dir, &from, &to).unwrap();
        let paths: Vec<_> = changes.iter().map(|c| (c.path.as_str(), c.code)).collect();
        assert_eq!(paths, [("a", 'M'), ("b", 'A')]);
        assert_eq!(files.len(), 2);
        assert_eq!(count_commits(dir, &format!("{from}..{to}")), 2);
        assert!(range_diff(dir, "no-such", &to).is_err());
    }

    #[test]
    fn lists_stashes() {
        let fx = Fx::new();
        let dir = &fx.repo;
        assert_eq!(stash_list(dir), ("No stashes".into(), Vec::new()));
        write(dir, "a", "changed\n");
        git(dir, &["stash", "push", "-q", "-m", "first"]);
        write(dir, "a", "changed again\n");
        git(dir, &["stash", "push", "-q", "-m", "second"]);
        let (list, oids) = stash_list(dir);
        let lines: Vec<&str> = list.lines().collect();
        assert_eq!(lines.len(), 2, "{list}");
        assert!(lines[0].starts_with("stash@{0} · "), "{list}");
        assert!(lines[0].ends_with("On main: second"), "{list}");
        assert!(lines[1].ends_with("On main: first"), "{list}");
        // A stash made since shifts `first` to stash@{2}: dropping
        // stash@{1} by its old commit is refused, by its new one it goes.
        write(dir, "a", "third\n");
        git(dir, &["stash", "push", "-q", "-m", "third"]);
        let first = StashSeen::Oid(oids[1].clone().unwrap());
        let changed = Failure::Blocked("the stash list changed; refresh and try again".into());
        assert_eq!(failure(stash_drop(dir, 1, &first)), changed);
        // Without a list read, the count seen guards it the same way.
        assert_eq!(failure(stash_drop(dir, 0, &StashSeen::Count(2))), changed);
        assert_eq!(stash_list(dir).0.lines().count(), 3);
        assert_eq!(failure(stash_pop(dir, 0, &StashSeen::Count(2))), changed);
        assert!(stash_drop(dir, 2, &first).is_ok());
        assert!(!stash_list(dir).0.contains("first"));
    }

    #[test]
    fn refs_and_names() {
        let fx = Fx::new();
        let dir = &fx.repo;
        assert!(has_ref(dir, "refs/heads/main"));
        assert!(has_ref(dir, "refs/remotes/origin/main"));
        assert!(!has_ref(dir, "refs/heads/nope"));
        assert!(has_ref(dir, "origin/main"));
        assert!(!has_ref(dir, "nope"));
        assert!(!has_ref(dir, "--all"));
        for good in ["feat/x", "fix-1", "ü"] {
            assert!(valid_branch(dir, good), "{good}");
        }
        for bad in [
            "", "a..b", "-x", "a b", "x.lock", "HEAD", "@{-1}", "a/", "~x",
        ] {
            assert!(!valid_branch(dir, bad), "{bad}");
        }
    }

    #[test]
    fn the_panel_judges_branch_names_as_git_does() {
        // Not `@`: `--branch` takes it for the branch checked out.
        let fx = Fx::new();
        let dir = &fx.repo;
        for name in [
            "feat/x",
            "fix-1",
            "ü",
            "a.b",
            "a@b",
            "x/y/z",
            "",
            "a..b",
            "-x",
            "a b",
            "x.lock",
            "HEAD",
            "@{-1}",
            "a@{b",
            "a/",
            "/a",
            "a//b",
            ".a",
            "a/.b",
            "a.",
            "~x",
            "a^",
            "a:b",
            "a?",
            "a*",
            "a[b",
            "a\\b",
            "a\tb",
            "a/x.lock/b",
        ] {
            assert_eq!(
                crate::repos::branch_name_problem(name).is_none(),
                valid_branch(dir, name),
                "{name:?}"
            );
        }
    }

    #[test]
    fn discarding_puts_files_back_as_head_has_them_and_deletes_new_ones() {
        let fx = Fx::local();
        let dir = &fx.repo;
        commit(dir, "kept", "kept");
        write(dir, "a", "changed");
        git(dir, &["add", "a"]);
        write(dir, "a", "changed again");
        write(dir, "kept", "changed too");
        write(dir, "new/one", "1");
        write(dir, "added", "x");
        git(dir, &["add", "added"]);
        let paths = ["a", "new/", "added"].map(String::from);
        discard(dir, &paths).unwrap();
        assert_eq!(read(dir, "a"), "first");
        assert!(!dir.join("new").exists() && !dir.join("added").exists());
        // Only those asked for.
        assert_eq!(read(dir, "kept"), "changed too");
        let status = git(dir, &["status", "--porcelain"]);
        assert_eq!(status.trim(), "M kept");
    }

    #[test]
    fn a_hung_hook_is_killed() {
        let fx = Fx::local();
        let hook = fx.script("hooks/pre-commit", "sleep 30");
        let hooks = hook.parent().unwrap().to_str().unwrap();
        git(&fx.repo, &["config", "core.hooksPath", hooks]);
        write(&fx.repo, "a", "changed");
        TIMEOUT.with(|t| t.set(Some(Duration::from_millis(300))));
        let started = Instant::now();
        let failed = commit_paths(&fx.repo, &["a".into()], "hung").unwrap_err();
        TIMEOUT.with(|t| t.set(None));
        assert_eq!(failed.failure, Failure::Timeout);
        assert!(failed.why.starts_with("timed out"), "{}", failed.why);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!failed.log.last().unwrap().ok);
        // The commit never happened.
        assert_eq!(git(&fx.repo, &["log", "-1", "--format=%s"]).trim(), "first");
    }

    #[test]
    fn a_stopped_command_is_killed_and_what_is_left_not_run() {
        let fx = Fx::local();
        let hook = fx.script("hooks/pre-commit", "sleep 30");
        let hooks = hook.parent().unwrap().to_str().unwrap();
        git(&fx.repo, &["config", "core.hooksPath", hooks]);
        write(&fx.repo, "a", "changed");
        let stop = Arc::new(AtomicBool::new(false));
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let heard = said.clone();
        watch(Some(Watch {
            stop: stop.clone(),
            say: Box::new(move |what| heard.lock().unwrap().push(what.into())),
        }));
        let stopper = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                stop.store(true, AtomicOrdering::Relaxed);
            })
        };
        let started = Instant::now();
        let failed = commit_paths(&fx.repo, &["a".into()], "hung").unwrap_err();
        stopper.join().unwrap();
        assert_eq!(failed.failure, Failure::Blocked(STOPPED.into()));
        assert!(started.elapsed() < Duration::from_secs(10));
        // Stopped already: nothing more runs.
        let failed = commit_paths(&fx.repo, &["a".into()], "again").unwrap_err();
        assert_eq!(failed.failure, Failure::Blocked(STOPPED.into()));
        assert!(failed.log.is_empty());
        watch(None);
        assert_eq!(git(&fx.repo, &["log", "-1", "--format=%s"]).trim(), "first");
        assert!(said.lock().unwrap().is_empty());
    }

    #[test]
    fn ssh_never_prompts() {
        let fx = Fx::local();
        let dir = &fx.repo;
        // Nothing listens on port 1: it fails at once, not hangs.
        git(dir, &["remote", "add", "far", "ssh://git@127.0.0.1:1/x"]);
        let started = Instant::now();
        let failed = push(dir, Some("far"), false).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(30));
        assert_ne!(failed.failure, Failure::Timeout);
        assert!(!failed.log[0].ok);
        assert_eq!(failed.log[0].command, "git push -u far HEAD");

        // An ssh that wants a key: Auth.
        let fake = fx.script(
            "no-key",
            "echo 'git@example.invalid: Permission denied (publickey).' >&2; exit 255",
        );
        git(dir, &["config", "core.sshCommand", fake.to_str().unwrap()]);
        git(
            dir,
            &["remote", "set-url", "far", "ssh://git@example.invalid/x"],
        );
        let failed = push(dir, Some("far"), false).unwrap_err();
        assert_eq!(failed.failure, Failure::Auth);
        assert!(fetch_all(dir).is_err());

        // A core.sshCommand of the user's that is ssh gets batch mode too.
        let ssh = fx.script("bin/ssh", "echo \"args: $*\" >&2; exit 255");
        let own = format!("{} -o Foo=bar", ssh.display());
        git(dir, &["config", "core.sshCommand", &own]);
        let failed = push(dir, Some("far"), false).unwrap_err();
        let said = &failed.log[0].output;
        assert!(said.contains("-o Foo=bar -o BatchMode=yes"), "{said}");
    }

    /// git's refusal to commit with nobody to commit as.
    const NO_IDENTITY: &str = "Author identity unknown\n\n*** Please tell me who you are.\n\n\
        Run\n\n  git config --global user.email \"you@example.com\"\n  \
        git config --global user.name \"Your Name\"\n\n\
        to set your account's default identity.\n\
        Omit --global to set the identity only in this repository.\n\n\
        fatal: unable to auto-detect email address (got 'me@box.(none)')\n";

    #[test]
    fn nobody_to_commit_as_is_an_identity_failure() {
        assert_eq!(classify(NO_IDENTITY, ""), Failure::Identity);
        let empty = "Committer identity unknown\n\nfatal: empty ident name (for <>) not allowed\n";
        assert_eq!(classify(empty, ""), Failure::Identity);
    }

    #[test]
    fn an_identity_needs_a_name_and_an_email_git_takes() {
        let who = |name: &str, email: &str| Identity {
            name: name.into(),
            email: email.into(),
        };
        assert!(who("", "a@b").problem().is_some());
        assert!(who("A <b>", "a@b").problem().is_some());
        assert!(who("A", "").problem().is_some());
        assert!(who("A", "ab").problem().is_some());
        assert!(who("A", "a b@c").problem().is_some());
        assert_eq!(who("Octo Cat", "o@c.at").problem(), None);
    }

    #[test]
    fn a_github_account_commits_as_its_name_else_login_and_noreply_address() {
        let user = serde_json::json!({"login": "octo", "id": 42, "name": null, "email": null});
        assert_eq!(
            github_identity(&user),
            Identity {
                name: "octo".into(),
                email: "42+octo@users.noreply.github.com".into(),
            }
        );
        let user =
            serde_json::json!({"login": "octo", "id": 42, "name": "Octo Cat", "email": "o@c.at"});
        assert_eq!(
            github_identity(&user),
            Identity {
                name: "Octo Cat".into(),
                email: "o@c.at".into(),
            }
        );
    }

    #[test]
    fn an_identity_set_in_the_repositories_only_is_read_back_there() {
        let (one, two) = (Fx::local(), Fx::local());
        let who = Identity {
            name: "Octo Cat".into(),
            email: "o@c.at".into(),
        };
        assert_eq!(identity(&one.repo), Identity::default());
        let roots = [one.repo.clone(), two.repo.clone()];
        set_identity(&roots, false, &who).unwrap();
        assert_eq!(identity(&one.repo), who);
        assert_eq!(identity(&two.repo), who);
        // Guessed from the first that knows, for one that doesn't.
        let three = Fx::local();
        let guess = guess_identity(
            std::slice::from_ref(&three.repo),
            std::slice::from_ref(&one.repo),
        );
        assert_eq!(guess, who);
    }
}
