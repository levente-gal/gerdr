//! What the repositories panel does to repositories: branch, commit, push,
//! fetch, pull, merge or rebase onto the default branch, park and resume
//! work, and see a conflicted operation through or abort it; and what it
//! checks before (whether a merge would conflict, what blocks a switch).
//!
//! Each runs `git` off the event loop, never waiting on a prompt: no
//! terminal for passwords (`GIT_TERMINAL_PROMPT=0`, ssh in batch mode), no
//! editor (`GIT_EDITOR=true`), English messages (`LC_ALL=C`) so that what
//! went wrong can be told from them, and a time limit. Each returns every
//! command it ran with what it printed, for the panel's log, and on
//! failure what kind it was, for the panel to offer what fixes it.
#![allow(dead_code)]

use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use crate::{
    desktop::shell_quote,
    git::{Change, InProgress},
};

/// How long a command that talks to a remote may take before it is killed.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a local one may take (a commit hook, a signing prompt).
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait between looks at whether a command has finished, at most.
const POLL: Duration = Duration::from_millis(20);
/// How long a command's output may still take to arrive once it has ended:
/// something it left running (a detached `gc`) may hold the pipe open.
const GRACE: Duration = Duration::from_secs(2);
/// How many lines of a diff are shown.
const DIFF_LINES: usize = 5000;
/// How many commits `log_ahead` shows without an upstream.
const LOG_LAST: &str = "10";

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
    /// folder); the repository is left mid-operation (or, for a stash,
    /// with the stash kept) to resolve.
    Conflict(Vec<String>),
    /// A password, passphrase or login was wanted.
    Auth,
    /// The remote has commits the branch doesn't: push refused.
    Rejected,
    /// The branch and its upstream both have commits: no fast-forward.
    Diverged,
    /// The branch is checked out in the worktree at this path.
    CheckedOut(PathBuf),
    /// Another git holds `index.lock`.
    Locked,
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

/// How the default branch's new commits come into a feature branch.
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
}

/// Whether a command talks to a remote: it gets longer, and ssh in batch
/// mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
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
}

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
        Reach::Local => LOCAL_TIMEOUT,
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
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        // Its own group, so that a kill takes its hooks and ssh too.
        .process_group(0);
    if reach == Reach::Network
        && let Some(ssh) = batch_ssh(dir)
    {
        cmd.env("GIT_SSH_COMMAND", ssh);
    }
    #[cfg(test)]
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env_remove("GIT_SSH")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    let finish =
        |stdout: String, stderr: String, code: Option<i32>, timed_out: Option<Duration>| {
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
            }
        };
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return finish(String::new(), format!("git: {err}\n"), None, None),
    };
    let stdout = Drain::new(child.stdout.take());
    let stderr = Drain::new(child.stderr.take());
    let timeout = limit(reach);
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    let kill = |child: &mut std::process::Child| {
        // Safety: signals the process group it leads.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.wait();
    };
    let (code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), None),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(POLL);
            }
            Ok(None) => {
                kill(&mut child);
                break (None, Some(timeout));
            }
            Err(_) => {
                kill(&mut child);
                break (None, None);
            }
        }
    };
    finish(stdout.finish(), stderr.finish(), code, timed_out)
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
    let out = exec(dir, args, Reach::Local);
    out.ran.ok.then_some(out.stdout)
}

/// What kind of failure git's output tells of. A conflict's files are
/// left for the caller to ask git for.
pub fn classify(stderr: &str, stdout: &str) -> Failure {
    let text = format!("{stderr}\n{stdout}");
    let has = |s: &str| text.contains(s);
    if let Some(path) = checked_out_at(&text) {
        return Failure::CheckedOut(path);
    }
    if has("index.lock") {
        return Failure::Locked;
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
/// checked out at '<path>'" or "is already used by worktree at '<path>'".
fn checked_out_at(text: &str) -> Option<PathBuf> {
    let (_, rest) = [
        "is already checked out at '",
        "is already used by worktree at '",
    ]
    .iter()
    .find_map(|s| text.split_once(s))?;
    let line = rest.lines().next()?;
    Some(PathBuf::from(line.rsplit_once('\'')?.0))
}

/// One line on why a command failed: the last line git marked as an
/// error, else the last line, with `fatal: `, `error: ` or `hint: `
/// stripped; from stderr, else stdout.
fn why(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
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

/// The files with unresolved conflicts, from the worktree's top folder.
fn conflicted(dir: &Path) -> Vec<String> {
    let out = query(dir, &["diff", "--name-only", "-z", "--diff-filter=U"]);
    out.unwrap_or_default()
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
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

    fn exec(&mut self, args: &[&str], reach: Reach) -> Result<String, Failed> {
        let out = exec(self.dir, args, reach);
        self.log.push(out.ran.clone());
        if out.ran.ok {
            return Ok(out.stdout);
        }
        Err(self.failed(&out))
    }

    /// The failure `out` tells of, with the log so far.
    fn failed(&mut self, out: &Output) -> Failed {
        let log = std::mem::take(&mut self.log);
        if let Some(after) = out.timed_out {
            let why = format!("timed out after {} s", after.as_secs_f32());
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

/// The worktree other than `dir`'s that has branch `name` checked out.
fn checked_out_elsewhere(dir: &Path, name: &str) -> Option<PathBuf> {
    let list = query(dir, &["worktree", "list", "--porcelain", "-z"])?;
    let here = top(dir);
    let want = format!("branch refs/heads/{name}");
    let mut path = None;
    for field in list.split('\0') {
        if let Some(p) = field.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if field == want {
            let p = path.take()?;
            let same =
                |a: &Path, b: &Path| a == b || a.canonicalize().ok() == b.canonicalize().ok();
            if !same(&p, &here) {
                return Some(p);
            }
        }
    }
    None
}

/// Where `dir`'s checkout stands now.
pub fn head(dir: &Path) -> Option<Head> {
    let sym = exec(
        dir,
        &["symbolic-ref", "-q", "--short", "HEAD"],
        Reach::Local,
    );
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

/// Start feature branch `name` from `base` (`origin/main`, fetched first
/// when it is a remote's): new and not tracking `base` (`--no-track`, so
/// its first push sets its own upstream); an existing `name` is only
/// switched to.
pub fn start_from(dir: &Path, name: &str, base: &str) -> OpResult {
    let mut op = Op::new(dir);
    if has_ref(dir, &format!("refs/heads/{name}")) {
        op.run(&["switch", name])?;
        return op.done();
    }
    // The longest remote name that prefixes it: remotes may have slashes.
    let remote = remotes(dir)
        .into_iter()
        .filter(|r| base.starts_with(&format!("{r}/")))
        .max_by_key(String::len);
    if let Some(remote) = remote {
        let branch = &base[remote.len() + 1..];
        op.net(&["fetch", &remote, branch])?;
    }
    op.run(&["switch", "--no-track", "-c", name, base])?;
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
    // `add` fails on a path matching nothing on disk or in the index, as
    // the old path of a rename staged with `git mv` is: that one is in
    // HEAD only, which `commit --only` looks in too.
    let listed = {
        let mut args = vec!["--literal-pathspecs", "ls-files", "-z", "--"];
        args.extend(paths.iter().map(String::as_str));
        query(&top, &args).unwrap_or_default()
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
    let mut args = vec!["--literal-pathspecs", "commit", "-m", msg, "--only", "--"];
    args.extend(paths.iter().map(String::as_str));
    op.run(&args)?;
    op.done()
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
    let lease = force_lease.then_some("--force-with-lease");
    if up_remote.is_some() && up_ref == format!("refs/heads/{branch}") {
        let args: Vec<&str> = ["push"].into_iter().chain(lease).collect();
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
        .chain(lease)
        .chain(["-u", &remote, "HEAD"])
        .collect();
    op.net(&args)?;
    op.done()
}

/// `fetch --all --prune`.
pub fn fetch_all(dir: &Path) -> OpResult {
    let mut op = Op::new(dir);
    op.net(&["fetch", "--all", "--prune"])?;
    op.done()
}

/// Pull the upstream's commits, `how`. A rebase or a merge stashes
/// uncommitted changes around it (`--autostash`); a fast-forward that
/// would touch them is refused instead, leaving all as it was.
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
        Pull::Rebase => &["pull", "--rebase", "--autostash"],
        Pull::Merge => &["pull", "--no-rebase", "--no-edit", "--autostash"],
    };
    op.net(args)?;
    op.done()
}

/// Bring `base` (`origin/main`) into the checked-out branch, `how`, with
/// `--autostash`; a merge with `--no-edit`. Blocked before the first
/// commit.
pub fn integrate(dir: &Path, base: &str, how: Integrate) -> OpResult {
    let mut op = Op::new(dir);
    if head_oid(dir).is_none() {
        return Err(op.blocked("no commits yet"));
    }
    match how {
        Integrate::Merge => op.run(&["merge", "--no-edit", "--autostash", base])?,
        Integrate::Rebase => op.run(&["rebase", "--autostash", base])?,
    };
    op.done()
}

/// The message a parked feature's stash has: `gerdr park <feature>`.
pub fn park_message(feature: &str) -> String {
    format!("gerdr park {feature}")
}

/// The newest stash `feature` was parked in: its commit and its current
/// `stash@{n}`. Others' stashes come and go, so `n` is only good now.
fn park_stash(dir: &Path, feature: &str) -> Option<(String, String)> {
    let out = query(dir, &["stash", "list", "--format=%H %gd %gs"])?;
    // The subject is "On <branch>: <message>".
    let tail = format!(": {}", park_message(feature));
    out.lines().find_map(|line| {
        let (sha, rest) = line.split_once(' ')?;
        let (at, subject) = rest.split_once(' ')?;
        subject
            .ends_with(&tail)
            .then(|| (sha.to_owned(), at.to_owned()))
    })
}

/// Drop the stash with commit `sha`, wherever it is in the stack now.
fn drop_stash(op: &mut Op, sha: &str) -> Result<(), Failed> {
    let out = query(op.dir, &["stash", "list", "--format=%H %gd"]).unwrap_or_default();
    let at = out
        .lines()
        .find_map(|l| l.strip_prefix(sha)?.strip_prefix(' '));
    if let Some(at) = at {
        op.run(&["stash", "drop", "-q", at])?;
    }
    Ok(())
}

/// Park the work on `feature`: stash every change, untracked files too,
/// as `park_message(feature)` (when there are any), then switch to
/// `default`. The stash stack is shared by the repository's worktrees, so
/// a stash is only ever found again by its message. When the switch fails
/// the work is put back.
pub fn park(dir: &Path, feature: &str, default: &str) -> OpResult {
    let mut op = Op::new(dir);
    // A default branch checked out in another worktree (the main one, from
    // a linked one) would stop the switch after the stash.
    if let Some(path) = checked_out_elsewhere(dir, default) {
        return Err(Failed {
            why: format!("{default} is checked out at {}", path.display()),
            failure: Failure::CheckedOut(path),
            log: Vec::new(),
        });
    }
    let changes = query(dir, &["status", "--porcelain", "--untracked-files=all"]);
    let sha = if changes.is_some_and(|c| !c.trim().is_empty()) {
        op.run(&["stash", "push", "-u", "-m", &park_message(feature)])?;
        park_stash(dir, feature).map(|(sha, _)| sha)
    } else {
        None
    };
    let switch = switch_args(dir, default);
    if let Err(mut failed) = op.run(&switch.iter().map(String::as_str).collect::<Vec<_>>()) {
        if let Some(sha) = sha {
            let mut back = Op::new(dir);
            if back.run(&["stash", "apply", "-q", &sha]).is_ok() {
                let _ = drop_stash(&mut back, &sha);
            }
            failed.log.extend(back.log);
        }
        return Err(failed);
    }
    op.done()
}

/// Resume `feature`: switch to it, then apply its park stash (found by
/// message, applied by commit id, then dropped, found again by commit).
/// On conflicts the stash is kept: `Failure::Conflict`, then
/// `finish_stash` or `abort_stash`. Without a park stash it only switches.
pub fn resume(dir: &Path, feature: &str) -> OpResult {
    let mut op = Op::new(dir);
    let switch = switch_args(dir, feature);
    op.run(&switch.iter().map(String::as_str).collect::<Vec<_>>())?;
    let Some((sha, _)) = park_stash(dir, feature) else {
        return op.done();
    };
    op.run(&["stash", "apply", &sha])?;
    drop_stash(&mut op, &sha)?;
    op.done()
}

/// After a resume's conflicts are resolved: unstage (`reset -q`) and drop
/// the feature's park stash. Files still conflicted stop it.
pub fn finish_stash(dir: &Path, feature: &str) -> OpResult {
    let files = conflicted(dir);
    if !files.is_empty() {
        return Err(Failed {
            why: format!("still conflicted: {}", files.join(", ")),
            failure: Failure::Conflict(files),
            log: Vec::new(),
        });
    }
    let mut op = Op::new(dir);
    op.run(&["reset", "-q"])?;
    if let Some((sha, _)) = park_stash(dir, feature) {
        drop_stash(&mut op, &sha)?;
    }
    op.done()
}

/// Give up a resume's conflicted stash apply (`reset --merge`); the stash
/// stays. The apply put the stash's untracked files back too, which the
/// reset leaves: those still as the stash has them go, so the next resume
/// doesn't trip over them.
pub fn abort_stash(dir: &Path, feature: &str) -> OpResult {
    let top = top(dir);
    let mut op = Op::new(&top);
    op.run(&["reset", "--merge"])?;
    if let Some((sha, _)) = park_stash(&top, feature) {
        let untracked = format!("{sha}^3");
        let tree = query(&top, &["ls-tree", "-r", "-z", &untracked]).unwrap_or_default();
        for entry in tree.split('\0').filter(|e| !e.is_empty()) {
            let Some((meta, path)) = entry.split_once('\t') else {
                continue;
            };
            let Some(blob) = meta.split_whitespace().nth(2) else {
                continue;
            };
            let file = top.join(path);
            let same = query(&top, &["hash-object", "--", path]).is_some_and(|h| h.trim() == blob);
            if same {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
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

/// Undo a batch: back to `before` (switch to its branch if it differs,
/// then `reset --keep` to its commit), only while HEAD is still at
/// `after`, else `Blocked("HEAD moved since")`.
pub fn undo(dir: &Path, before: &Head, after: &Head) -> OpResult {
    let mut op = Op::new(dir);
    if head(dir).as_ref() != Some(after) {
        return Err(op.blocked("HEAD moved since"));
    }
    let Some(oid) = &before.oid else {
        return Err(op.blocked("no commit to go back to"));
    };
    if before.branch != after.branch {
        match &before.branch {
            Some(branch) => op.run(&["switch", branch])?,
            None => {
                op.run(&["switch", "--detach", oid])?;
                return op.done();
            }
        };
    }
    op.run(&["reset", "--keep", oid])?;
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
    let out = exec(dir, &args, Reach::Local);
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
pub fn valid_branch(dir: &Path, name: &str) -> bool {
    // `--branch` would take `@{-1}` for the branch before; `HEAD` and a
    // leading dash would read as something else.
    if name.is_empty() || name.starts_with('-') || name == "HEAD" || name.contains("@{") {
        return false;
    }
    query(dir, &["check-ref-format", "--branch", name]).is_some()
}

/// `text` cut to `DIFF_LINES` lines, saying how many more there were.
fn cap(text: String) -> String {
    let total = text.lines().count();
    if total <= DIFF_LINES {
        return text;
    }
    let mut out: String = text
        .lines()
        .take(DIFF_LINES)
        .flat_map(|l| [l, "\n"])
        .collect();
    out.push_str(&format!("… {} more lines\n", total - DIFF_LINES));
    out
}

/// The diff of `change` against HEAD, for the detail pane; an untracked
/// file as all added, an untracked folder as its file list.
pub fn diff(dir: &Path, change: &Change) -> String {
    let top = top(dir);
    let path = change.path.as_str();
    let out = if change.code == '?' && path.ends_with('/') {
        let args = [
            "--literal-pathspecs",
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            path,
        ];
        exec(&top, &args, Reach::Local)
    } else if change.code == '?' {
        let args = [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-index",
            "--",
            "/dev/null",
            path,
        ];
        exec(&top, &args, Reach::Local)
    } else {
        let from = head_oid(&top).unwrap_or_else(|| empty_tree(&top));
        let mut args = vec![
            "--literal-pathspecs",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-M",
            &from,
            "--",
        ];
        args.extend(change.orig.as_deref());
        args.push(path);
        exec(&top, &args, Reach::Local)
    };
    // `diff --no-index` exits 1 when the files differ, as they do.
    match out.code {
        Some(0) => cap(out.stdout),
        Some(1) if change.code == '?' && !out.stdout.is_empty() => cap(out.stdout),
        _ => why(&out.stderr, &out.stdout),
    }
}

/// The commits the upstream doesn't have yet, one line each (`log
/// --oneline @{u}..`); without an upstream, the last few.
pub fn log_ahead(dir: &Path) -> String {
    let base = ["log", "--oneline", "--decorate=short", "--no-color"];
    let out = if query(dir, &["rev-parse", "-q", "--verify", "@{upstream}"]).is_some() {
        query(dir, &[&base[..], &["@{upstream}.."]].concat())
    } else {
        query(dir, &[&base[..], &["-n", LOG_LAST]].concat())
    };
    out.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        ];
        for (stderr, stdout, want, want_why) in samples {
            assert_eq!(&classify(stderr, stdout), want, "{stderr}{stdout}");
            assert_eq!(why(stderr, stdout), *want_why, "{stderr}{stdout}");
        }
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
        // A park there stops before stashing anything.
        write(&wt, "a", "changed");
        let failed = park(&wt, "wtb", "main").unwrap_err();
        assert_eq!(failed.failure, Failure::CheckedOut(fx.repo.clone()));
        assert!(failed.log.is_empty());
        assert_eq!(read(&wt, "a"), "changed");
        assert_eq!(branch(&wt).as_deref(), Some("wtb"));
    }

    #[test]
    fn start_a_feature_from_the_fresh_default_branch() {
        let fx = Fx::new();
        let other = fx.clone("other");
        commit(&other, "b", "upstream work");
        git(&other, &["push", "-q"]);
        let log = start_from(&fx.repo, "feat/x", "origin/main").unwrap();
        let commands: Vec<_> = log.iter().map(|r| r.command.as_str()).collect();
        assert_eq!(
            commands,
            [
                "git fetch origin main",
                "git switch --no-track -c feat/x origin/main"
            ]
        );
        assert_eq!(rev(&fx.repo, "HEAD"), rev(&other, "HEAD"));
        assert!(query(&fx.repo, &["rev-parse", "-q", "--verify", "@{upstream}"]).is_none());
        // Its first push makes its own upstream.
        push(&fx.repo, None, false).unwrap();
        let up = git(&fx.repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert_eq!(up.trim(), "origin/feat/x");
        // Once there, it is only switched to.
        git(&fx.repo, &["switch", "-q", "main"]);
        let log = start_from(&fx.repo, "feat/x", "origin/main").unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].command, "git switch feat/x");
        // A local base isn't fetched.
        let log = start_from(&fx.repo, "feat/y", "main").unwrap();
        assert_eq!(log[0].command, "git switch --no-track -c feat/y main");
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
        assert_eq!(push(dir, None, false).unwrap()[0].command, "git push");

        // Rewritten: refused, then pushed with a lease.
        git(dir, &["commit", "-q", "--amend", "-m", "two again"]);
        assert_eq!(failure(push(dir, None, false)), Failure::Rejected);
        let log = push(dir, None, true).unwrap();
        assert_eq!(log[0].command, "git push --force-with-lease");
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

        // A merge, with a change stashed around it.
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
            "git merge --no-edit --autostash main"
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

    /// The stash stack, as `<sha> <subject>` lines.
    fn stashes(dir: &Path) -> Vec<String> {
        let out = git(dir, &["stash", "list", "--format=%H %gs"]);
        out.lines().map(str::to_owned).collect()
    }

    #[test]
    fn park_and_resume_leave_others_stashes_alone() {
        let fx = Fx::local();
        let dir = &fx.repo;
        write(dir, "a", "someone's");
        git(dir, &["stash", "push", "-q", "-m", "someone's work"]);
        git(dir, &["switch", "-q", "-c", "feat"]);
        commit(dir, "f", "feat");
        write(dir, "f", "changed");
        write(dir, "u/new.txt", "untracked");
        write(dir, "s", "staged");
        git(dir, &["add", "s"]);

        let log = park(dir, "feat", "main").unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].command, "git stash push -u -m 'gerdr park feat'");
        assert_eq!(branch(dir).as_deref(), Some("main"));
        assert!(git(dir, &["status", "--porcelain"]).is_empty());
        // More of someone else's on top: ours isn't stash@{0} any more.
        write(dir, "a", "later");
        git(dir, &["stash", "push", "-q", "-m", "later work"]);
        let before = stashes(dir);
        assert_eq!(before.len(), 3);
        assert!(before[1].ends_with(": gerdr park feat"));

        resume(dir, "feat").unwrap();
        assert_eq!(branch(dir).as_deref(), Some("feat"));
        assert_eq!(read(dir, "f"), "changed");
        assert_eq!(read(dir, "u/new.txt"), "untracked");
        assert_eq!(read(dir, "s"), "staged");
        assert_eq!(stashes(dir), [before[0].clone(), before[2].clone()]);

        // Nothing to park: only a switch; nothing parked: the same.
        git(dir, &["stash", "push", "-q", "-u", "-m", "clear it"]);
        let log = park(dir, "feat", "main").unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].command, "git switch main");
        let log = resume(dir, "feat").unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(stashes(dir).len(), 3);
    }

    #[test]
    fn a_resume_that_conflicts() {
        let fx = Fx::local();
        let dir = &fx.repo;
        git(dir, &["switch", "-q", "-c", "feat"]);
        write(dir, "a", "parked");
        write(dir, "n", "new");
        park(dir, "feat", "main").unwrap();
        // Meanwhile feat moved on: the parked change no longer applies.
        git(dir, &["switch", "-q", "feat"]);
        commit(dir, "a", "committed");
        git(dir, &["switch", "-q", "main"]);

        assert_eq!(
            failure(resume(dir, "feat")),
            Failure::Conflict(vec!["a".into()])
        );
        assert!(park_stash(dir, "feat").is_some());
        assert_eq!(
            failure(finish_stash(dir, "feat")),
            Failure::Conflict(vec!["a".into()])
        );
        // The apply brought the untracked file back; aborting takes it away.
        assert_eq!(read(dir, "n"), "new");
        abort_stash(dir, "feat").unwrap();
        assert_eq!(read(dir, "a"), "committed");
        assert!(git(dir, &["status", "--porcelain"]).is_empty());
        assert!(park_stash(dir, "feat").is_some());

        assert_eq!(
            failure(resume(dir, "feat")),
            Failure::Conflict(vec!["a".into()])
        );
        write(dir, "a", "resolved");
        mark_resolved(dir, "a").unwrap();
        finish_stash(dir, "feat").unwrap();
        assert!(park_stash(dir, "feat").is_none());
        assert_eq!(read(dir, "a"), "resolved");
        assert_eq!(read(dir, "n"), "new");
        assert_eq!(git(dir, &["status", "--porcelain"]), " M a\n?? n\n");
    }

    #[test]
    fn undo_a_batch_unless_head_moved() {
        let fx = Fx::local();
        let dir = &fx.repo;
        let before = head(dir).unwrap();
        start_from(dir, "feat", "main").unwrap();
        commit(dir, "f", "feat");
        let after = head(dir).unwrap();
        undo(dir, &before, &after).unwrap();
        assert_eq!(head(dir), Some(before.clone()));

        commit(dir, "b", "batch");
        let after = head(dir).unwrap();
        write(dir, "a", "uncommitted");
        commit(dir, "c", "since");
        let failed = undo(dir, &before, &after).unwrap_err();
        assert_eq!(failed.failure, Failure::Blocked("HEAD moved since".into()));
        assert!(failed.log.is_empty());
        git(dir, &["reset", "-q", "--keep", "HEAD~1"]);
        undo(dir, &before, &after).unwrap();
        assert_eq!(head(dir), Some(before.clone()));
        // reset --keep keeps what wasn't committed.
        assert_eq!(read(dir, "a"), "uncommitted");

        let unborn = Head {
            branch: Some("main".into()),
            oid: None,
        };
        let now = head(dir).unwrap();
        assert_eq!(
            failure(undo(dir, &unborn, &now)),
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
        write(dir, "a", "changed\n");
        let d = diff(dir, &change("a", None, 'M'));
        assert!(d.contains("-first") && d.contains("+changed"), "{d}");
        write(dir, "new file", "hello\n");
        let d = diff(dir, &change("new file", None, '?'));
        assert!(d.contains("new file mode") && d.contains("+hello"), "{d}");
        write(dir, "u/one", "1");
        write(dir, "u/two/three", "3");
        assert_eq!(diff(dir, &change("u/", None, '?')), "u/one\nu/two/three\n");
        commit(dir, "old", "moved\n");
        git(dir, &["mv", "old", "moved"]);
        let d = diff(dir, &change("moved", Some("old"), 'R'));
        assert!(
            d.contains("rename from old") && d.contains("rename to moved"),
            "{d}"
        );
        std::fs::remove_file(dir.join("a")).unwrap();
        let d = diff(dir, &change("a", None, 'D'));
        assert!(d.contains("deleted file mode"), "{d}");

        let big: String = (0..6000).map(|n| format!("{n}\n")).collect();
        write(dir, "big", &big);
        let d = diff(dir, &change("big", None, '?'));
        assert_eq!(d.lines().count(), DIFF_LINES + 1);
        assert!(d.ends_with("… 1006 more lines\n"), "{}", &d[d.len() - 40..]);

        git(&fx.root, &["init", "-q", "-b", "main", "unborn"]);
        let unborn = fx.root.join("unborn");
        write(&unborn, "x", "x\n");
        git(&unborn, &["add", "x"]);
        let d = diff(&unborn, &change("x", None, 'A'));
        assert!(d.contains("+x"), "{d}");
    }

    #[test]
    fn logs_ahead() {
        let fx = Fx::new();
        let dir = &fx.repo;
        assert_eq!(log_ahead(dir), "");
        commit(dir, "b", "second");
        commit(dir, "c", "third");
        let log = log_ahead(dir);
        assert_eq!(log.lines().count(), 2);
        assert!(log.lines().next().unwrap().ends_with("third"), "{log}");
        switch_or_create(dir, "lonely").unwrap();
        assert_eq!(log_ahead(dir).lines().count(), 3);
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
}
