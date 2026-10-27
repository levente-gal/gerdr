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
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use crate::{
    desktop::shell_quote,
    diff::{self, FileDiff},
    git::{self, Change, InProgress},
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
/// How many lines of a repository's diff are read.
const REPO_DIFF_LINES: usize = 20_000;
/// How many of an untracked folder's files are listed.
const FOLDER_FILES: usize = 200;
/// An untracked file bigger than this isn't shown.
const UNTRACKED_BYTES: u64 = 1 << 20;
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
/// checked out at '<path>'", "is already used by worktree at '<path>'" or
/// "cannot delete branch 'x' used by worktree at '<path>'".
fn checked_out_at(text: &str) -> Option<PathBuf> {
    let (_, rest) = ["is already checked out at '", "used by worktree at '"]
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
/// or tag has: what deleting it loses. Zero when its changes are all in a
/// default branch already, squashed or rebased there (see `git::merged`).
/// `None` when git can't tell.
pub fn unmerged_commits(dir: &Path, name: &str) -> Option<usize> {
    let exclude = format!("--exclude={name}");
    let args = [
        "rev-list",
        "--count",
        &format!("refs/heads/{name}"),
        "--not",
        &exclude,
        "--branches",
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
            Some(1) => return Err(op.blocked("1 commit only on it")),
            Some(n) => return Err(op.blocked(&format!("{n} commits only on it"))),
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
    let top = top(dir);
    let mut tracked = Vec::new();
    let mut failed = None;
    if changes.iter().any(|c| c.code != '?') {
        let from = head_oid(&top).unwrap_or_else(|| empty_tree(&top));
        let args = [
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-M",
            &from,
        ];
        let out = exec(&top, &args, Reach::Local);
        match out.code {
            Some(0) => tracked = diff::parse(&cap(out.stdout, REPO_DIFF_LINES)),
            _ => failed = Some(why(&out.stderr, &out.stdout)),
        }
    }
    changes
        .iter()
        .map(|change| {
            if change.code == '?' {
                return untracked(&top, change);
            }
            let found = tracked.iter().position(|f| f.path == change.path);
            let mut file = found.map_or_else(FileDiff::default, |i| tracked.swap_remove(i));
            if found.is_none() {
                let why = failed.clone();
                file.notes
                    .push(why.unwrap_or_else(|| "No changes to show".into()));
            }
            file.path = change.path.clone();
            file.orig = change.orig.clone();
            file.code = change.code;
            file
        })
        .collect()
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
        let out = exec(top, &args, Reach::Local);
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
    file
}

/// The commits the upstream doesn't have yet, one line each (`log
/// --oneline @{u}..`), under a line saying so; without an upstream, or
/// with nothing to push, the last few.
pub fn log_ahead(dir: &Path) -> String {
    let base = ["log", "--oneline", "--decorate=short", "--no-color"];
    let last = || query(dir, &[&base[..], &["-n", LOG_LAST]].concat()).unwrap_or_default();
    if query(dir, &["rev-parse", "-q", "--verify", "@{upstream}"]).is_none() {
        return format!("No upstream yet. The last commits:\n{}", last());
    }
    let ahead = query(dir, &[&base[..], &["@{upstream}.."]].concat()).unwrap_or_default();
    if ahead.trim().is_empty() {
        return format!("Nothing to push. The last commits:\n{}", last());
    }
    format!("To push:\n{ahead}")
}

/// The repository's stashes, newest first, one line each: `stash@{0} ·
/// 2 hours ago · WIP on main: 1a2b3c4 msg`; else a line saying there are
/// none.
pub fn stash_list(dir: &Path) -> String {
    let list = query(dir, &["stash", "list", "--format=%gd · %cr · %gs"]).unwrap_or_default();
    if list.trim().is_empty() {
        return "No stashes".into();
    }
    list
}

/// Stash the uncommitted changes, untracked files too.
pub fn stash_push(dir: &Path) -> OpResult {
    let mut op = Op::new(dir);
    op.run(&["stash", "push", "--include-untracked"])?;
    op.done()
}

/// Bring the newest stash back and drop it; a conflict keeps the stash.
pub fn stash_pop(dir: &Path) -> OpResult {
    let mut op = Op::new(dir);
    op.run(&["stash", "pop"])?;
    op.done()
}

/// Drop the newest stash.
pub fn stash_drop(dir: &Path) -> OpResult {
    let mut op = Op::new(dir);
    op.run(&["stash", "drop"])?;
    op.done()
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
            Failure::Blocked("1 commit only on it".into())
        );
        assert!(has_ref(r, "refs/heads/feat"));
        // Pushed, its commits are kept on the remote.
        git(r, &["push", "-q", "origin", "feat"]);
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
        assert_eq!(d[0].notes, ["… 4000 more lines"]);

        git(&fx.root, &["init", "-q", "-b", "main", "unborn"]);
        let unborn = fx.root.join("unborn");
        write(&unborn, "x", "x\n");
        git(&unborn, &["add", "x"]);
        let d = repo_diff(&unborn, &[change("x", None, 'A')]);
        assert_eq!(lines(&d[0]), ["+x"]);
    }

    #[test]
    fn logs_ahead() {
        let fx = Fx::new();
        let dir = &fx.repo;
        let log = log_ahead(dir);
        assert!(log.starts_with("Nothing to push."), "{log}");
        assert_eq!(log.lines().count(), 2);
        commit(dir, "b", "second");
        commit(dir, "c", "third");
        let log = log_ahead(dir);
        assert_eq!(log.lines().count(), 3);
        assert!(log.lines().nth(1).unwrap().ends_with("third"), "{log}");
        switch_or_create(dir, "lonely").unwrap();
        let log = log_ahead(dir);
        assert!(log.starts_with("No upstream yet."), "{log}");
        assert_eq!(log.lines().count(), 4);
    }

    #[test]
    fn lists_stashes() {
        let fx = Fx::new();
        let dir = &fx.repo;
        assert_eq!(stash_list(dir), "No stashes");
        write(dir, "a", "changed\n");
        git(dir, &["stash", "push", "-q", "-m", "first"]);
        write(dir, "a", "changed again\n");
        git(dir, &["stash", "push", "-q", "-m", "second"]);
        let list = stash_list(dir);
        let lines: Vec<&str> = list.lines().collect();
        assert_eq!(lines.len(), 2, "{list}");
        assert!(lines[0].starts_with("stash@{0} · "), "{list}");
        assert!(lines[0].ends_with("On main: second"), "{list}");
        assert!(lines[1].ends_with("On main: first"), "{list}");
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
