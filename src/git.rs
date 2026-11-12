//! Git status of the workspace folders, shown in the sidebar: the branch,
//! whether there are uncommitted changes, and how far the branch is ahead of
//! and behind its upstream, and the repository's other worktrees, which
//! agents tend to leave behind. `git status` can take a while in a big
//! repository, so it runs off the event loop.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

/// Where a folder's repository stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// The checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Some file is changed, staged or untracked.
    pub dirty: bool,
    /// Commits the upstream doesn't have yet.
    pub ahead: u32,
    /// Upstream commits not merged here yet.
    pub behind: u32,
    /// The repository's worktrees but the one the folder is in.
    pub worktrees: Vec<Worktree>,
    /// The worktree the folder is in, when the repository has others.
    pub here: Option<Worktree>,
    /// The top folder of the worktree the folder is in, which the others'
    /// paths are shown from.
    pub root: Option<PathBuf>,
    /// The commit checked out; `None` before the first commit.
    pub head: Option<String>,
    /// The changed, staged, untracked and conflicted files, in `git status`
    /// order.
    pub changes: Vec<Change>,
    /// The branch's upstream, as `origin/feat/x`; `None` without one.
    pub upstream: Option<String>,
    /// The remote the repository's branches go to: the upstream's, else
    /// `origin`, else its only remote; `None` without a remote.
    pub remote: Option<String>,
    /// The remote's default branch, as `main` (no remote prefix): what
    /// `<remote>/HEAD` points at, else a local `main` or `master`.
    pub default_branch: Option<String>,
    /// The worktree's own git folder (`.git`, or `.git/worktrees/<name>`
    /// for a linked worktree), absolute.
    pub git_dir: Option<PathBuf>,
    /// The git folder its worktrees share, absolute: two checkouts with the
    /// same one are the same repository.
    pub common_dir: Option<PathBuf>,
    /// A merge, rebase, cherry-pick or revert stopped half-way in it.
    pub op: Option<InProgress>,
    /// How many stashes the repository has (shared by its worktrees).
    pub stashes: u32,
    /// When it last fetched (`FETCH_HEAD`'s time); `None` if never.
    pub fetched: Option<SystemTime>,
    /// The local branches, by name, in git's order.
    pub local_branches: Vec<String>,
    /// The remote's branches, by name without the remote (`feat/x`), in
    /// git's order, its `HEAD` left out.
    pub remote_branches: Vec<String>,
    /// When each branch was last committed to, by name: the newer of its
    /// local and remote tips' commit times, in seconds since the epoch.
    pub branch_times: HashMap<String, i64>,
}

/// A file `git status` lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// From the worktree's top folder, as git prints it with `-z` (not
    /// quoted). An untracked folder ends in `/`.
    pub path: String,
    /// Where a renamed or copied file was.
    pub orig: Option<String>,
    /// What happened to it, as one letter: `M` changed, `A` added, `D`
    /// deleted, `R` renamed, `C` copied, `T` type changed, `U` conflicted,
    /// `?` untracked.
    pub code: char,
}

impl Change {
    pub fn conflicted(&self) -> bool {
        self.code == 'U'
    }
}

/// An operation stopped half-way, waiting to be continued or aborted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InProgress {
    Merge,
    /// At commit `step` of `total`.
    Rebase {
        step: u32,
        total: u32,
    },
    CherryPick,
    Revert,
}

/// A worktree of the repository a folder is in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The commit checked out.
    pub head: Option<String>,
    /// The checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// It is the main worktree, the one the others' branches merge into.
    pub main: bool,
    /// Some file in it is changed, staged or untracked.
    pub dirty: bool,
    /// Some file git tracks in it is changed or staged; untracked ones
    /// aside, which a checkout leaves where they are.
    pub edited: bool,
    /// Its branch's changes are all in the main worktree's HEAD or a
    /// default branch, merged, squashed or picked (see `merged`): likely
    /// done with.
    pub merged: bool,
    /// Its branch has no commit of its own yet: it is at the main
    /// worktree's HEAD or a default branch. Not merged, as there is nothing
    /// to merge, but deleting it loses no commit.
    pub fresh: bool,
    /// Its folder is gone; `git worktree prune` would drop it.
    pub gone: bool,
}

impl Worktree {
    /// What it is called: its branch, else its folder's name.
    pub fn name(&self) -> String {
        self.branch.clone().unwrap_or_else(|| {
            let dir = self.path.file_name().unwrap_or(self.path.as_os_str());
            format!("{} (detached)", dir.to_string_lossy())
        })
    }

    /// Whether it still needs a look: changes in it, a branch with commits
    /// not merged (or none), or its folder gone. A merged, clean one is
    /// done with; a new one with no commit yet has nothing to look at.
    pub fn needs_attention(&self) -> bool {
        self.dirty || self.attention_tag().is_some()
    }

    /// Why it needs a look, in a word, besides changes (its dot).
    pub fn attention_tag(&self) -> Option<&'static str> {
        if self.gone {
            Some("gone")
        } else if !self.main && !self.merged && !self.fresh {
            Some("unmerged")
        } else {
            None
        }
    }

    /// Where it stands, in a word or two: why it needs a look, else "main
    /// worktree", "new" or "merged".
    pub fn tag(&self) -> &'static str {
        self.attention_tag().unwrap_or(if self.main {
            "main worktree"
        } else if self.fresh {
            "new"
        } else {
            "merged"
        })
    }
}

impl GitStatus {
    /// All the repository's worktrees, the main one first; empty when it
    /// has no others.
    pub fn all_worktrees(&self) -> Vec<&Worktree> {
        let mut all: Vec<_> = self.here.iter().chain(&self.worktrees).collect();
        all.sort_by_key(|w| !w.main);
        all
    }

    /// The main worktree's folder: which repository this is.
    pub fn main_path(&self) -> Option<&Path> {
        let main = self.here.iter().chain(&self.worktrees).find(|w| w.main)?;
        Some(&main.path)
    }
}

/// The status of the repository `dir` is in; `None` outside one or without git.
///
/// It runs every few seconds for every workspace folder (as `read_all`),
/// so it keeps to five git commands where it can and reads git's own files
/// for the rest.
#[cfg(test)]
pub fn status(dir: &Path) -> Option<GitStatus> {
    let mut status = own_status(dir)?;
    (status.here, status.worktrees) = split_worktrees(worktrees(dir), status.root.as_deref());
    Some(status)
}

/// `status` but for the worktrees, which are the same list from any of
/// them: `read_all` reads them once per repository.
fn own_status(dir: &Path) -> Option<GitStatus> {
    // -z: paths come unquoted, whatever is in them. --show-stash counts
    // the stashes from git itself, whichever ref store it uses.
    let out = git(
        dir,
        &["status", "--porcelain=v2", "--branch", "--show-stash", "-z"],
    )?;
    let mut status = parse(&out);
    let folders = git(
        dir,
        &[
            "rev-parse",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
    );
    if let Some(folders) = folders {
        let mut lines = folders.lines();
        status.root = lines.next().map(PathBuf::from);
        status.git_dir = lines.next().map(PathBuf::from);
        status.common_dir = lines.next().map(|common| absolute(dir, common));
    }
    status.op = status.git_dir.as_deref().and_then(in_progress);
    status.fetched = status.common_dir.as_deref().and_then(fetched);
    let remotes = git(dir, &["remote"]).unwrap_or_default();
    let remotes: Vec<&str> = remotes.lines().filter(|r| !r.is_empty()).collect();
    status.remote = pick_remote(&remotes, status.upstream.as_deref());
    (
        status.local_branches,
        status.remote_branches,
        status.branch_times,
    ) = branches(dir, status.remote.as_deref());
    status.default_branch = default_branch(
        dir,
        status.common_dir.as_deref(),
        status.remote.as_deref(),
        &status.local_branches,
    );
    Some(status)
}

/// `path` as git printed it for a command run in `dir`, absolute. The
/// symlinks are resolved before any `..`, as git itself saw it: taking
/// `..` off by hand goes wrong when `dir` is reached through a symlink.
fn absolute(dir: &Path, path: &str) -> PathBuf {
    let path = dir.join(path);
    path.canonicalize().unwrap_or(path)
}

/// The operation stopped half-way in the worktree whose git folder is
/// `git_dir`, from the files git keeps while it waits. A rebase comes
/// first: its picks and merges may leave their own files as it stops.
fn in_progress(git_dir: &Path) -> Option<InProgress> {
    let number = |dir: &Path, file: &str| -> u32 {
        std::fs::read_to_string(dir.join(file))
            .ok()
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0)
    };
    // rebase-merge for the merge backend (the default), rebase-apply for
    // the apply one (and `git am`).
    for (folder, step, total) in [
        ("rebase-merge", "msgnum", "end"),
        ("rebase-apply", "next", "last"),
    ] {
        let folder = git_dir.join(folder);
        if folder.is_dir() {
            return Some(InProgress::Rebase {
                step: number(&folder, step),
                total: number(&folder, total),
            });
        }
    }
    [
        ("MERGE_HEAD", InProgress::Merge),
        ("CHERRY_PICK_HEAD", InProgress::CherryPick),
        ("REVERT_HEAD", InProgress::Revert),
    ]
    .into_iter()
    .find(|(file, _)| git_dir.join(file).is_file())
    .map(|(_, op)| op)
}

/// When the repository whose shared git folder is `common_dir` last
/// fetched. Each worktree writes `FETCH_HEAD` in its own git folder, but
/// what a fetch brings is shared, so the newest of them counts.
fn fetched(common_dir: &Path) -> Option<SystemTime> {
    let linked = std::fs::read_dir(common_dir.join("worktrees")).into_iter();
    let linked = linked.flatten().flatten().map(|entry| entry.path());
    std::iter::once(common_dir.to_path_buf())
        .chain(linked)
        .filter_map(|dir| dir.join("FETCH_HEAD").metadata().ok()?.modified().ok())
        .max()
}

/// The remote among `remotes` the branches go to: `upstream`'s, else
/// `origin`, else the only one. Remote names may have a `/` in them, so the
/// longest that `upstream` starts with wins.
fn pick_remote(remotes: &[&str], upstream: Option<&str>) -> Option<String> {
    let of_upstream = upstream.and_then(|upstream| {
        remotes
            .iter()
            .filter(|r| {
                upstream
                    .strip_prefix(**r)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|r| r.len())
    });
    let remote = of_upstream
        .or_else(|| remotes.iter().find(|r| **r == "origin"))
        .or_else(|| (remotes.len() == 1).then(|| &remotes[0]))?;
    Some((*remote).to_owned())
}

/// The local branches of the repository `dir` is in, and `remote`'s, by
/// name, and when each was last committed to.
fn branches(dir: &Path, remote: Option<&str>) -> (Vec<String>, Vec<String>, HashMap<String, i64>) {
    let theirs = remote.map(|remote| format!("refs/remotes/{remote}/"));
    let mut args = vec![
        "for-each-ref",
        "--format=%(refname) %(committerdate:unix)",
        "refs/heads",
    ];
    // The pattern matches whole folders: `origin` isn't `origin-x`.
    if let Some(theirs) = &theirs {
        args.push(theirs.trim_end_matches('/'));
    }
    let out = git(dir, &args).unwrap_or_default();
    let mut local = Vec::new();
    let mut remote = Vec::new();
    let mut times = HashMap::new();
    for line in out.lines() {
        // Git allows no spaces in a ref's name.
        let (line, time) = line.split_once(' ').unwrap_or((line, ""));
        let name = if let Some(name) = line.strip_prefix("refs/heads/") {
            local.push(name.to_owned());
            name
        } else if let Some(name) = theirs.as_deref().and_then(|t| line.strip_prefix(t))
            && name != "HEAD"
        {
            remote.push(name.to_owned());
            name
        } else {
            continue;
        };
        if let Ok(time) = time.parse::<i64>() {
            let newest = times.entry(name.to_owned()).or_insert(time);
            *newest = time.max(*newest);
        }
    }
    (local, remote, times)
}

/// The default branch of the repository `dir` is in, whose shared git
/// folder is `common_dir`: what `remote`'s HEAD points at, else a local
/// `main`, else `master` (from `local`, its local branches).
fn default_branch(
    dir: &Path,
    common_dir: Option<&Path>,
    remote: Option<&str>,
    local: &[String],
) -> Option<String> {
    // The files ref store keeps symbolic refs as files of their own, never
    // packed, and a branch just committed to as one too; a reftable store
    // (or a packed branch) needs git.
    let files = common_dir.filter(|common| !common.join("reftable").exists());
    if let Some(remote) = remote {
        let head = format!("refs/remotes/{remote}/HEAD");
        let target = match files {
            Some(common) => std::fs::read_to_string(common.join(&head))
                .ok()
                .and_then(|r| Some(r.trim().strip_prefix("ref: refs/remotes/")?.to_owned())),
            None => git(dir, &["symbolic-ref", "-q", "--short", &head]),
        };
        let branch = target
            .as_deref()
            .and_then(|t| t.trim().strip_prefix(remote)?.strip_prefix('/'))
            .filter(|b| !b.is_empty());
        if let Some(branch) = branch {
            return Some(branch.to_owned());
        }
    }
    ["main", "master"]
        .into_iter()
        .find(|name| local.iter().any(|b| b == name))
        .map(str::to_owned)
}

/// What `git args` prints in `dir`, or what it says went wrong.
fn git_run(dir: &Path, args: &[&str]) -> Result<String, String> {
    // Its callers remove worktrees, delete branches and switch: writers
    // that must take index.lock.
    let out = command(dir, args)
        .env_remove("GIT_OPTIONAL_LOCKS")
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("git: {err}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    // The last line is the one that says why; the ones before it are hints.
    let why = err
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let why = why
        .trim()
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ");
    Err(if why.is_empty() {
        format!("git {} failed", args.first().unwrap_or(&""))
    } else {
        why.to_owned()
    })
}

/// A git of `args` in `dir`, on gerdr's own settings: never prompting, no
/// `GIT_DIR`-style variables of a caller in the way, and no optional locks,
/// so a read never takes index.lock from the user or an agent. A command
/// that must take that lock (a commit, a checkout) removes
/// `GIT_OPTIONAL_LOCKS` itself.
pub fn command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR");
    #[cfg(test)]
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t");
    cmd
}

/// What `git args` prints in `dir`; `None` when it fails or without git.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    // `command` leaves index.lock alone, so a refresh never gets in the
    // way of the user's own git commands.
    let out = command(dir, args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The worktrees of the repository `dir` is in, the main one first; none
/// when it has no others.
fn worktrees(dir: &Path) -> Vec<Worktree> {
    let Some(list) = git(dir, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let mut all = parse_worktrees(&list);
    if all.len() < 2 {
        return Vec::new();
    }
    // The first is the main worktree: what the others' branches merge into.
    all[0].main = true;
    let main_head = all[0].head.clone();
    let also: Vec<&str> = main_head.as_deref().into_iter().collect();
    let mut targets = merge_targets(dir);
    targets.extend(main_head.clone());
    for w in &mut all {
        let branch = !w.main && w.branch.is_some();
        w.merged = branch
            && w.head
                .as_deref()
                .is_some_and(|head| merged(dir, head, &also));
        w.fresh = branch && w.head.as_ref().is_some_and(|head| targets.contains(head));
        let changes = (!w.gone)
            .then(|| git(&w.path, &["status", "--porcelain"]))
            .flatten()
            .unwrap_or_default();
        w.dirty = !changes.is_empty();
        w.edited = changes.lines().any(|l| !l.starts_with("??"));
    }
    all
}

/// Of worktrees `all`, the one at `here` and the others.
fn split_worktrees(
    mut all: Vec<Worktree>,
    here: Option<&Path>,
) -> (Option<Worktree>, Vec<Worktree>) {
    let at = all.iter().position(|w| Some(w.path.as_path()) == here);
    (at.map(|i| all.remove(i)), all)
}

/// Whether commit `rev`'s changes are all in a default branch (see
/// `merge_targets`) or in one of commits `also`: it is in one, or merging
/// it there would change nothing (it was squashed, rebased or picked in),
/// or a commit there makes the very changes it makes, not undone since (it
/// was squashed in, then changed again). One that changes nothing at all
/// isn't. What git says is kept per pair of commits, as those don't change.
pub fn merged(dir: &Path, rev: &str, also: &[&str]) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<(String, String), bool>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    let rev = format!("{rev}^{{commit}}");
    let Some(rev) = git(dir, &["rev-parse", "-q", "--verify", &rev]) else {
        return false;
    };
    let rev = rev.trim();
    let mut targets = merge_targets(dir);
    targets.extend(also.iter().map(|t| t.to_string()));
    targets.iter().filter(|t| t.as_str() != rev).any(|target| {
        let key = (target.clone(), rev.to_owned());
        if let Some(&known) = seen.lock().unwrap().get(&key) {
            return known;
        }
        let merged = merged_into(dir, target, rev);
        seen.lock().unwrap().insert(key, merged);
        merged
    })
}

/// Whether commit `rev`'s changes are all in commit `target` (see `merged`).
fn merged_into(dir: &Path, target: &str, rev: &str) -> bool {
    if git(dir, &["merge-base", "--is-ancestor", rev, target]).is_some() {
        return true;
    }
    let tree = |rev: &str| git(dir, &["rev-parse", &format!("{}^{{tree}}", rev.trim())]);
    let Some(base) = git(dir, &["merge-base", target, rev]) else {
        return false;
    };
    let base = base.trim();
    if tree(base) == tree(rev) {
        return false;
    }
    // Exit 1 on a conflict: no answer then.
    let merge = git(dir, &["merge-tree", "--write-tree", target, rev]);
    if merge.is_some_and(|m| m.lines().next() == tree(target).as_deref().map(str::trim)) {
        return true;
    }
    picked(dir, target, base, rev)
}

/// Whether a commit of `target`'s since `base` makes the very changes
/// `rev` makes since then, as a squash of it does, and none undoes them:
/// their patch ids, which leave line numbers out, match.
fn picked(dir: &Path, target: &str, base: &str, rev: &str) -> bool {
    // A commit of `rev`'s changes on `base`, and one undoing them on top.
    let commit = |tree: &str, parent: &str| {
        let tree = format!("{tree}^{{tree}}");
        let args = [
            "-c",
            "user.name=gerdr",
            "-c",
            "user.email=gerdr@localhost",
            "commit-tree",
            "--no-gpg-sign",
            &tree,
            "-p",
            parent,
            "-m",
            "gerdr",
        ];
        git(dir, &args).map(|c| c.trim().to_owned())
    };
    let Some(does) = commit(rev, base) else {
        return false;
    };
    let Some(undoes) = commit(base, &does) else {
        return false;
    };
    // `=` marks one of them a commit of `target`'s matches.
    let range = format!("{target}...{undoes}");
    let args = [
        "rev-list",
        "--right-only",
        "--cherry-mark",
        "--no-merges",
        &range,
    ];
    let Some(marks) = git(dir, &args) else {
        return false;
    };
    let marked = |commit: &str| marks.lines().any(|l| l.strip_prefix('=') == Some(commit));
    marked(&does) && !marked(&undoes)
}

/// The default branches others merge into, as commits: what each remote's
/// HEAD points at, and any local or remote `main` or `master`.
fn merge_targets(dir: &Path) -> Vec<String> {
    let args = [
        "for-each-ref",
        "--format=%(objectname) %(refname) %(symref)",
        "refs/remotes",
        "refs/heads/main",
        "refs/heads/master",
    ];
    let mut targets: Vec<String> = Vec::new();
    for line in git(dir, &args).unwrap_or_default().lines() {
        let mut fields = line.split(' ');
        let (Some(commit), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        let symref = fields.next().unwrap_or("");
        let default = !symref.is_empty() || name.ends_with("/main") || name.ends_with("/master");
        if default && !targets.iter().any(|t| t == commit) {
            targets.push(commit.to_owned());
        }
    }
    targets
}

/// Remove worktree `w` of the repository whose main worktree is at `main`,
/// and its branch once merged or when it has no commit of its own, or
/// anyway when `force`: its changes, and its commits not merged, are lost
/// then. A gone one is pruned.
pub fn remove_worktree(main: &Path, w: &Worktree, force: bool) -> Result<(), String> {
    if w.main {
        return Err("the main worktree stays".into());
    }
    // Merged or fresh is from a status read seconds old: a commit since
    // would go with the branch.
    if let Some(branch) = &w.branch
        && !force
        && (w.merged || w.fresh)
    {
        let tip = git_run(
            main,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        )?;
        if Some(tip.trim()) != w.head.as_deref() {
            return Err("the branch moved since; refresh and try again".into());
        }
    }
    if w.gone {
        git_run(main, &["worktree", "prune"])?;
    } else {
        let path = w.path.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path);
        git_run(main, &args)?;
    }
    if let Some(branch) = &w.branch
        && (w.merged || w.fresh || force)
    {
        // -D: -d takes a squashed branch for unmerged.
        git_run(main, &["branch", "-D", branch])?;
    }
    Ok(())
}

/// Why worktree `w`'s branch can't be checked out in the repository's main
/// worktree `main` now, if anything stops it: changes to tracked files
/// either side would stay behind or come along. Untracked ones don't stop
/// it: `w`'s are brought along or left behind (see `untracked`), and git
/// refuses to switch over main's if the branch has the same file.
pub fn checkout_check(main: &Worktree, w: &Worktree) -> Result<(), String> {
    let name = w.name();
    if w.main {
        return Err("This is the main worktree".into());
    }
    if w.branch.is_none() {
        return Err(format!("{name} has no branch"));
    }
    if w.gone {
        return Err(format!("{name}'s folder is gone"));
    }
    if w.edited {
        return Err(format!("{name} has uncommitted changes"));
    }
    if main.edited {
        return Err(format!("{} has uncommitted changes", main.name()));
    }
    Ok(())
}

/// Check worktree `w`'s branch out in the main worktree at `main`, leaving
/// `w` on its commit, detached: Git has a branch checked out in one
/// worktree at a time. `w` gets its branch back if the main one can't take
/// it.
pub fn checkout_in_main(main: &Path, w: &Worktree) -> Result<(), String> {
    let Some(branch) = &w.branch else {
        return Err(format!("{} has no branch", w.name()));
    };
    git_run(&w.path, &["switch", "--detach"])?;
    if let Err(why) = git_run(main, &["switch", branch]) {
        let _ = git_run(&w.path, &["switch", branch]);
        return Err(why);
    }
    Ok(())
}

/// An untracked file or folder of a worktree, for a checkout to bring
/// along or leave behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Untracked {
    /// From the worktree's top folder; a folder, all of it untracked, ends
    /// in `/`.
    pub path: String,
    /// The untracked files in it, for a folder; 1 for a file.
    pub files: usize,
    /// It is a symbolic link.
    pub link: bool,
}

/// The untracked files and folders in worktree `dir`, ignored ones left
/// out, in git's order.
pub fn untracked(dir: &Path) -> Vec<Untracked> {
    let Some(out) = git(dir, &["status", "--porcelain=v2", "-z"]) else {
        return Vec::new();
    };
    parse(&out)
        .changes
        .into_iter()
        .filter(|c| c.code == '?')
        .map(|c| {
            let files = if c.path.ends_with('/') {
                let args = ["ls-files", "--others", "--exclude-standard", "-z", "--"];
                let listed = git(dir, &[&args[..], &[c.path.as_str()]].concat());
                listed.map_or(0, |l| l.split('\0').filter(|f| !f.is_empty()).count())
            } else {
                1
            };
            let link = std::fs::symlink_metadata(dir.join(c.path.trim_end_matches('/')))
                .is_ok_and(|m| m.file_type().is_symlink());
            Untracked {
                path: c.path,
                files,
                link,
            }
        })
        .collect()
}

/// Which of `paths` (as `untracked` gives them) something in folder `main`
/// is at already, so bringing them along would overwrite it.
pub fn clashes(main: &Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|p| std::fs::symlink_metadata(main.join(p.trim_end_matches('/'))).is_ok())
        .cloned()
        .collect()
}

/// Move `paths` (as `untracked` gives them) from worktree `from` to the
/// same place in worktree `to`, making the folders they go in; how many
/// files that moves. It stops at the first one that can't go, and says
/// which are left in `from`. One something is at in `to` stays: nothing
/// is overwritten.
pub fn bring_untracked(from: &Path, to: &Path, paths: &[Untracked]) -> Result<usize, String> {
    let mut moved = 0;
    for (i, u) in paths.iter().enumerate() {
        let path = u.path.trim_end_matches('/');
        let (src, dst) = (from.join(path), to.join(path));
        let result = if std::fs::symlink_metadata(&dst).is_ok() {
            Err(format!("something is at {path} already"))
        } else {
            dst.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::rename(&src, &dst))
                .map_err(|e| e.to_string())
        };
        if let Err(why) = result {
            let left: Vec<&str> = paths[i..].iter().map(|u| u.path.as_str()).collect();
            return Err(format!("{why}; left behind: {}", left.join(", ")));
        }
        moved += u.files;
    }
    Ok(moved)
}

/// Why worktree `w` can't be squashed into the main worktree at `main` now,
/// if anything stops it.
pub fn squash_check(main: &Path, w: &Worktree) -> Result<(), String> {
    let name = w.name();
    if w.main {
        return Err("This is the main worktree".into());
    }
    let Some(_) = &w.branch else {
        return Err(format!("{name} has no branch"));
    };
    if w.gone {
        return Err(format!("{name}'s folder is gone"));
    }
    if w.merged {
        return Err(format!("{name} is merged already"));
    }
    if w.fresh {
        return Err(format!("{name} has no commit of its own"));
    }
    if w.edited {
        return Err(format!("{name} has uncommitted changes"));
    }
    let status = git_run(main, &["status", "--porcelain=v2", "--branch", "-z"])?;
    let main_status = parse(&status);
    let Some(target) = main_status.branch else {
        return Err("The main worktree has no branch checked out".into());
    };
    if main_status.changes.iter().any(|c| c.code != '?') {
        return Err(format!("{target} has uncommitted changes"));
    }
    Ok(())
}

/// Read the status of each of `dirs` on a thread of its own, all at once,
/// and hand those in a repository to `send`: a round takes as long as the
/// slowest folder, not all of them one after another. The worktrees are
/// read once per repository, however many of the folders are in it.
pub fn read_all(
    dirs: Vec<PathBuf>,
    send: impl FnOnce(HashMap<PathBuf, GitStatus>) + Send + 'static,
) {
    std::thread::spawn(move || {
        let statuses: Vec<(PathBuf, GitStatus)> = std::thread::scope(|scope| {
            let reads: Vec<_> = dirs
                .into_iter()
                .map(|dir| scope.spawn(move || Some((own_status(&dir)?, dir))))
                .collect();
            reads
                .into_iter()
                .filter_map(|read| read.join().ok().flatten())
                .map(|(status, dir)| (dir, status))
                .collect()
        });
        // A folder git gave no shared git folder for stands alone.
        let repo = |dir: &Path, status: &GitStatus| -> PathBuf {
            status.common_dir.clone().unwrap_or_else(|| dir.to_owned())
        };
        let mut repos: Vec<(PathBuf, &Path)> = Vec::new();
        for (dir, status) in &statuses {
            let common = repo(dir, status);
            if !repos.iter().any(|(c, _)| *c == common) {
                repos.push((common, dir));
            }
        }
        let lists: HashMap<PathBuf, Vec<Worktree>> = std::thread::scope(|scope| {
            let reads: Vec<_> = repos
                .into_iter()
                .map(|(common, dir)| scope.spawn(move || (common, worktrees(dir))))
                .collect();
            reads
                .into_iter()
                .filter_map(|read| read.join().ok())
                .collect()
        });
        let read = statuses
            .into_iter()
            .map(|(dir, mut status)| {
                let all = lists.get(&repo(&dir, &status)).cloned().unwrap_or_default();
                (status.here, status.worktrees) = split_worktrees(all, status.root.as_deref());
                (dir, status)
            })
            .collect();
        send(read)
    });
}

/// The web page of the repository `dir` is in: its `origin` remote's, as
/// an https address; `None` without one or without git.
pub fn web_url(dir: &Path) -> Option<String> {
    let out = command(dir, &["remote", "get-url", "origin"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    remote_to_web(String::from_utf8_lossy(&out.stdout).trim())
}

/// The https address of the web page of the repository at `remote`:
/// `git@host:owner/repo.git`, `ssh://git@host:22/owner/repo` or
/// `https://user@host/owner/repo.git` all give `https://host/owner/repo`.
/// `None` for a local path.
fn remote_to_web(remote: &str) -> Option<String> {
    let (host, path) = match remote.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(scheme, "https" | "http" | "ssh" | "git" | "git+ssh") {
                return None;
            }
            rest.split_once('/')?
        }
        // scp-like: [user@]host:path
        None => remote.split_once(':')?,
    };
    // Without the user, and but for http(s) without the port: ssh's isn't
    // the web's.
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = if remote.starts_with("http") {
        host
    } else {
        host.split(':').next()?
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    if host.is_empty() || path.is_empty() || host.contains('/') || host.starts_with('.') {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

/// The worktrees in `git worktree list --porcelain` output: a block per
/// worktree, its lines "worktree <path>", "HEAD <oid>", then "branch
/// refs/heads/<name>" or "detached", maybe "prunable <why>".
fn parse_worktrees(out: &str) -> Vec<Worktree> {
    let mut all: Vec<Worktree> = Vec::new();
    for line in out.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            all.push(Worktree {
                path: value.into(),
                ..Worktree::default()
            });
            continue;
        }
        let Some(w) = all.last_mut() else { continue };
        match key {
            "HEAD" => w.head = Some(value.into()),
            "branch" => {
                w.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into());
            }
            "prunable" => w.gone = true,
            _ => {}
        }
    }
    all
}

/// The status in `git status --porcelain=v2 --branch -z` output, records
/// ended by NULs: "# " headers about the branch (and the stashes, with
/// `--show-stash`), then one per changed, untracked or conflicted file. A
/// renamed or copied file's record is followed by one of its old path.
fn parse(out: &str) -> GitStatus {
    let mut status = GitStatus::default();
    let mut records = out.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            parse_header(&mut status, header);
            continue;
        }
        // The path is last and may hold spaces, so it is what is left after
        // the fields before it: 8 for a changed file, 9 for a renamed one,
        // 10 for a conflicted one.
        let path = |n: usize| record.splitn(n + 1, ' ').nth(n);
        let field = |n: usize| record.split(' ').nth(n).unwrap_or("");
        let change = match field(0) {
            "1" => path(8).map(|path| (path, None, code(field(1)))),
            "2" => path(9).map(|path| {
                let orig = records.next().map(str::to_owned);
                let copied = field(8).starts_with('C');
                (path, orig, if copied { 'C' } else { 'R' })
            }),
            "u" => path(10).map(|path| (path, None, 'U')),
            "?" => path(1).map(|path| (path, None, '?')),
            // Ignored files ("!"), and the empty end after the last NUL.
            _ => None,
        };
        if let Some((path, orig, code)) = change {
            status.changes.push(Change {
                path: path.to_owned(),
                orig,
                code,
            });
        }
    }
    status.dirty = !status.changes.is_empty();
    status
}

/// Take in one "# " header of `git status --porcelain=v2` output.
fn parse_header(status: &mut GitStatus, header: &str) {
    let (key, value) = header.split_once(' ').unwrap_or((header, ""));
    match key {
        "branch.oid" => status.head = (value != "(initial)").then(|| value.to_owned()),
        "branch.head" => status.branch = (value != "(detached)").then(|| value.to_owned()),
        "branch.upstream" => status.upstream = Some(value.to_owned()),
        "branch.ab" => {
            for count in value.split_whitespace() {
                if let Some(n) = count.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = count.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        }
        "stash" => status.stashes = value.parse().unwrap_or(0),
        _ => {}
    }
}

/// The letter for a changed file's `XY`, what happened to it in the index
/// and in the worktree, `.` for nothing: the one that says most.
fn code(xy: &str) -> char {
    ['A', 'D', 'T']
        .into_iter()
        .find(|c| xy.contains(*c))
        .unwrap_or('M')
}

#[cfg(test)]
mod tests {
    use super::{
        Change, GitStatus, InProgress, Worktree, bring_untracked, checkout_check, checkout_in_main,
        clashes, parse, parse_worktrees, pick_remote, remote_to_web, remove_worktree, squash_check,
        status, untracked,
    };
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    /// A repository of its own under the system's temporary folder, on
    /// branch main with a first commit, its worktrees beside it; removed
    /// when dropped.
    struct Repo(PathBuf);

    impl Repo {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-git-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(dir.join("repo")).unwrap();
            let repo = Self(dir.canonicalize().unwrap().join("repo"));
            repo.git(&repo.0, &["init", "-q", "-b", "main"]);
            repo.commit(&repo.0, "a", "first");
            repo
        }

        /// Run git in `dir`, with no config of the user's in the way.
        fn git(&self, dir: &Path, args: &[&str]) {
            assert!(self.try_git(dir, args), "git {args:?} failed");
        }

        /// Run git in `dir` like `git`, for a command meant to stop on a
        /// conflict: whether it went through.
        fn try_git(&self, dir: &Path, args: &[&str]) -> bool {
            super::command(dir, args)
                .env_remove("GIT_OPTIONAL_LOCKS")
                .output()
                .unwrap()
                .status
                .success()
        }

        /// A folder beside the repository, for another one or a remote.
        fn beside(&self, name: &str) -> PathBuf {
            self.0.with_file_name(name)
        }

        /// Commit `file` with `message` as its text on a new branch `name`
        /// off main, then go back to main.
        fn branch_commit(&self, name: &str, file: &str, message: &str) {
            self.git(&self.0, &["switch", "-q", "-c", name]);
            self.commit(&self.0, file, message);
            self.git(&self.0, &["switch", "-q", "main"]);
        }

        /// A bare clone beside it, added as its remote `origin`.
        fn bare_remote(&self) -> PathBuf {
            let remote = self.beside("remote.git");
            let (from, to) = (self.0.to_str().unwrap(), remote.to_str().unwrap());
            self.git(&self.0, &["clone", "-q", "--bare", from, to]);
            self.git(&self.0, &["remote", "add", "origin", to]);
            remote
        }

        /// Write `file` in `dir` and commit it.
        fn commit(&self, dir: &Path, file: &str, message: &str) {
            std::fs::write(dir.join(file), message).unwrap();
            self.git(dir, &["add", file]);
            self.git(dir, &["commit", "-q", "-m", message]);
        }

        /// A worktree on new branch `name`, with a commit of its own.
        fn worktree(&self, name: &str) -> PathBuf {
            let path = self.0.with_file_name(name);
            let p = path.to_str().unwrap();
            self.git(&self.0, &["worktree", "add", "-q", "-b", name, p]);
            self.commit(&path, name, name);
            path
        }

        fn find(&self, name: &str) -> Worktree {
            let status = status(&self.0).unwrap();
            let all = status.worktrees.into_iter().chain(status.here);
            all.into_iter()
                .find(|w| w.branch.as_deref() == Some(name))
                .unwrap()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn a_branch_with_no_commit_of_its_own_is_fresh() {
        let repo = Repo::new();
        let path = repo.0.with_file_name("new");
        let p = path.to_str().unwrap();
        repo.git(&repo.0, &["worktree", "add", "-q", "-b", "new", p]);
        let new = repo.find("new");
        assert!(new.fresh && !new.merged && !new.main);
        assert!(!repo.find("main").fresh);
        assert_eq!(new.tag(), "new");
        assert!(!new.needs_attention());
        assert_eq!(
            squash_check(&repo.0, &new),
            Err("new has no commit of its own".into())
        );
        // It goes, branch and all, without force.
        assert_eq!(remove_worktree(&repo.0, &new, false), Ok(()));
        assert!(!path.exists());
        let branches = super::git(&repo.0, &["branch", "--list", "new"]).unwrap();
        assert!(branches.trim().is_empty(), "{branches}");
        // A commit of its own makes it not.
        repo.worktree("topic");
        assert!(!repo.find("topic").fresh);
    }

    #[test]
    fn a_commit_since_the_read_stops_a_delete() {
        let repo = Repo::new();
        let path = repo.0.with_file_name("new");
        let p = path.to_str().unwrap();
        repo.git(&repo.0, &["worktree", "add", "-q", "-b", "new", p]);
        let stale = repo.find("new");
        assert!(stale.fresh);
        repo.commit(&path, "new", "fresh work");
        assert!(remove_worktree(&repo.0, &stale, false).is_err());
        assert!(path.exists());
        let branches = super::git(&repo.0, &["branch", "--list", "new"]).unwrap();
        assert!(!branches.trim().is_empty());
    }

    #[test]
    fn a_squashed_branch_is_merged() {
        let repo = Repo::new();
        repo.worktree("topic");
        let topic = repo.find("topic");
        assert!(!topic.merged && !topic.dirty && !topic.main);
        assert!(repo.find("main").main);
        assert_eq!(squash_check(&repo.0, &topic), Ok(()));
        repo.git(&repo.0, &["merge", "-q", "--squash", "topic"]);
        repo.git(&repo.0, &["commit", "-q", "-m", "squash topic"]);
        let topic = repo.find("topic");
        assert!(topic.merged);
        assert!(!topic.needs_attention());
        assert!(squash_check(&repo.0, &topic).is_err());
        // Still when its lines are changed again since.
        repo.commit(&repo.0, "topic", "changed since");
        assert!(repo.find("topic").merged);
        // And it goes, branch and all.
        assert_eq!(remove_worktree(&repo.0, &topic, false), Ok(()));
        assert!(!topic.path.exists());
        assert!(status(&repo.0).unwrap().worktrees.is_empty());
    }

    #[test]
    fn changes_stop_a_delete_unless_forced() {
        let repo = Repo::new();
        let path = repo.worktree("wip");
        std::fs::write(path.join("scratch"), "x").unwrap();
        let (wip, main) = (repo.find("wip"), repo.find("main"));
        assert!(wip.dirty && !wip.edited && !wip.merged);
        // An untracked file doesn't stop a squash; an edited one does.
        assert_eq!(squash_check(&repo.0, &wip), Ok(()));
        std::fs::write(path.join("wip"), "edited").unwrap();
        let wip = repo.find("wip");
        assert!(wip.edited);
        assert!(squash_check(&repo.0, &wip).is_err());
        assert!(remove_worktree(&repo.0, &wip, false).is_err());
        assert!(path.exists());
        assert_eq!(remove_worktree(&repo.0, &wip, true), Ok(()));
        assert!(!path.exists());
        assert!(remove_worktree(&repo.0, &main, true).is_err());
    }

    #[test]
    fn a_worktrees_branch_is_checked_out_in_the_main_one() {
        let repo = Repo::new();
        let path = repo.worktree("topic");
        let (topic, main) = (repo.find("topic"), repo.find("main"));
        assert_eq!(checkout_check(&main, &topic), Ok(()));
        assert!(checkout_check(&main, &main).is_err());
        assert_eq!(checkout_in_main(&repo.0, &topic), Ok(()));
        let status = status(&repo.0).unwrap();
        assert_eq!(status.branch.as_deref(), Some("topic"));
        // The worktree stays on the commit, detached.
        let left = status.worktrees.iter().find(|w| w.path == path).unwrap();
        assert_eq!(left.branch, None);
        assert_eq!(left.head, topic.head);
        // Changes to tracked files either side stop it.
        std::fs::write(path.join("scratch"), "x").unwrap();
        let edited = Worktree {
            dirty: true,
            edited: true,
            ..topic.clone()
        };
        assert!(checkout_check(&main, &edited).is_err());
        let main = Worktree {
            dirty: true,
            edited: true,
            ..main
        };
        assert!(checkout_check(&main, &topic).is_err());
    }

    #[test]
    fn untracked_files_come_along_or_stay() {
        let repo = Repo::new();
        let path = repo.worktree("topic");
        std::fs::create_dir_all(path.join("dev/new")).unwrap();
        std::fs::write(path.join("dev/a.py"), "a").unwrap();
        std::fs::write(path.join("dev/new/b.py"), "b").unwrap();
        std::fs::write(path.join("dev/new/c.py"), "c").unwrap();
        std::os::unix::fs::symlink("/nowhere", path.join("venv")).unwrap();
        std::fs::write(repo.0.join(".git/info/exclude"), "*.log\n").unwrap();
        std::fs::write(path.join("out.log"), "ignored").unwrap();
        let (topic, main) = (repo.find("topic"), repo.find("main"));
        assert!(topic.dirty && !topic.edited);
        assert_eq!(checkout_check(&main, &topic), Ok(()));
        let found = untracked(&path);
        let shown: Vec<(&str, usize, bool)> = found
            .iter()
            .map(|u| (u.path.as_str(), u.files, u.link))
            .collect();
        assert_eq!(shown, [("dev/", 3, false), ("venv", 1, true)]);
        // One main has already stays put, and so do those after it.
        std::fs::write(repo.0.join("dev"), "mine").unwrap();
        let paths: Vec<String> = found.iter().map(|u| u.path.clone()).collect();
        assert_eq!(clashes(&repo.0, &paths), ["dev/"]);
        let refused = bring_untracked(&path, &repo.0, &found).unwrap_err();
        assert!(refused.ends_with("left behind: dev/, venv"), "{refused}");
        assert!(path.join("dev/a.py").exists());
        std::fs::remove_file(repo.0.join("dev")).unwrap();
        assert!(clashes(&repo.0, &paths).is_empty());
        checkout_in_main(&repo.0, &topic).unwrap();
        assert_eq!(bring_untracked(&path, &repo.0, &found), Ok(4));
        assert_eq!(
            std::fs::read_to_string(repo.0.join("dev/new/c.py")).unwrap(),
            "c"
        );
        assert!(std::fs::symlink_metadata(repo.0.join("venv")).is_ok());
        assert!(!path.join("dev").exists());
        // Ignored files stay.
        assert!(path.join("out.log").exists());
        assert!(untracked(&path).is_empty());
    }

    #[test]
    fn a_failed_checkout_gives_the_worktree_its_branch_back() {
        let repo = Repo::new();
        let path = repo.worktree("topic");
        let topic = repo.find("topic");
        // An untracked file in main that topic tracks stops the switch.
        std::fs::write(repo.0.join("topic"), "mine").unwrap();
        assert!(checkout_in_main(&repo.0, &topic).is_err());
        assert_eq!(status(&repo.0).unwrap().branch.as_deref(), Some("main"));
        assert_eq!(status(&path).unwrap().branch.as_deref(), Some("topic"));
    }

    #[test]
    fn remote_pages() {
        let page = Some("https://github.com/o/r".to_owned());
        assert_eq!(remote_to_web("git@github.com:o/r.git"), page);
        assert_eq!(remote_to_web("github.com:o/r"), page);
        assert_eq!(remote_to_web("ssh://git@github.com:22/o/r.git"), page);
        assert_eq!(remote_to_web("ssh://github.com/o/r/"), page);
        assert_eq!(remote_to_web("https://github.com/o/r.git"), page);
        assert_eq!(remote_to_web("https://user@github.com/o/r"), page);
        assert_eq!(
            remote_to_web("https://git.example.com:8443/group/sub/r.git").as_deref(),
            Some("https://git.example.com:8443/group/sub/r")
        );
        for local in ["/srv/git/r.git", "file:///srv/git/r.git", "../r", "r"] {
            assert_eq!(remote_to_web(local), None, "{local}");
        }
    }

    /// `records` as `git status -z` prints them, each ended by a NUL.
    fn z(records: &[&str]) -> String {
        records.iter().map(|r| format!("{r}\0")).collect()
    }

    /// A change of `code` at `path`.
    fn change(code: char, path: &str) -> Change {
        Change {
            path: path.into(),
            orig: None,
            code,
        }
    }

    #[test]
    fn clean_branch_with_upstream() {
        let out = z(&[
            "# branch.oid 1a2b",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "# stash 3",
        ]);
        assert_eq!(
            parse(&out),
            GitStatus {
                branch: Some("main".into()),
                dirty: false,
                ahead: 2,
                behind: 1,
                worktrees: Vec::new(),
                here: None,
                root: None,
                head: Some("1a2b".into()),
                upstream: Some("origin/main".into()),
                stashes: 3,
                ..GitStatus::default()
            }
        );
    }

    #[test]
    fn changes_make_it_dirty() {
        let out = z(&[
            "# branch.oid 1a2b",
            "# branch.head topic",
            "1 .M N... 100644 100644 100644 a b src/main.rs",
        ]);
        assert_eq!(
            parse(&out),
            GitStatus {
                branch: Some("topic".into()),
                dirty: true,
                ahead: 0,
                behind: 0,
                worktrees: Vec::new(),
                here: None,
                root: None,
                head: Some("1a2b".into()),
                changes: vec![change('M', "src/main.rs")],
                ..GitStatus::default()
            }
        );
        assert!(parse(&z(&["# branch.head main", "? notes.txt"])).dirty);
        // Ignored files aren't changes.
        assert!(!parse(&z(&["# branch.head main", "! target/"])).dirty);
    }

    #[test]
    fn detached_head_and_new_repo() {
        let detached = parse(&z(&["# branch.oid 1a2b", "# branch.head (detached)"]));
        assert_eq!(detached.branch, None);
        assert_eq!(detached.head.as_deref(), Some("1a2b"));
        let new = parse(&z(&["# branch.oid (initial)", "# branch.head master"]));
        assert_eq!(new.branch.as_deref(), Some("master"));
        assert_eq!(new.head, None);
    }

    #[test]
    fn each_kind_of_change() {
        let out = z(&[
            "# branch.oid 1a2b",
            "# branch.head main",
            "1 A. N... 000000 100644 100644 0 b new file.rs",
            "1 AM N... 000000 100644 100644 0 b added then changed",
            "1 .D N... 100644 100644 000000 a a gone",
            "1 MD N... 100644 100644 000000 a b changed then gone",
            "1 T. N... 100644 120000 120000 a b link",
            "1 M. N... 100644 100644 100644 a b staged",
            "2 R. N... 100644 100644 100644 a a R100 new name\twith tab",
            "old name",
            "2 C. N... 100644 100644 100644 a a C75 copy",
            "original",
            "u UU N... 100644 100644 100644 100644 a b c both changed.rs",
            "u AA N... 000000 100644 100644 100644 0 b c both added",
            "? dir/",
            "? line\nbreak ünï",
            "! ignored",
        ]);
        let renamed = |code, path: &str, orig: &str| Change {
            orig: Some(orig.into()),
            ..change(code, path)
        };
        assert_eq!(
            parse(&out).changes,
            vec![
                change('A', "new file.rs"),
                change('A', "added then changed"),
                change('D', "gone"),
                change('D', "changed then gone"),
                change('T', "link"),
                change('M', "staged"),
                renamed('R', "new name\twith tab", "old name"),
                renamed('C', "copy", "original"),
                change('U', "both changed.rs"),
                change('U', "both added"),
                change('?', "dir/"),
                change('?', "line\nbreak ünï"),
            ]
        );
        assert!(parse(&out).changes[8].conflicted());
    }

    #[test]
    fn the_remote_branches_go_to() {
        let remotes = ["fork", "origin", "team", "team/sub"];
        let pick = |remotes: &[&str], upstream| pick_remote(remotes, upstream);
        assert_eq!(pick(&remotes, Some("fork/x")).as_deref(), Some("fork"));
        // The longest remote name the upstream starts with.
        assert_eq!(
            pick(&remotes, Some("team/sub/feat/x")).as_deref(),
            Some("team/sub")
        );
        assert_eq!(pick(&remotes, Some("team/x")).as_deref(), Some("team"));
        // No upstream, or one that's a local branch: origin.
        assert_eq!(pick(&remotes, None).as_deref(), Some("origin"));
        assert_eq!(pick(&remotes, Some("main")).as_deref(), Some("origin"));
        assert_eq!(pick(&remotes, Some("forked/x")).as_deref(), Some("origin"));
        // Else the only one.
        assert_eq!(pick(&["fork"], None).as_deref(), Some("fork"));
        assert_eq!(pick(&["fork", "team"], None), None);
        assert_eq!(pick(&[], Some("origin/main")), None);
    }

    #[test]
    fn worktree_blocks() {
        let out = "worktree /src/gerdr\nHEAD 1a2b\nbranch refs/heads/main\n\n\
                   worktree /src/gerdr/.claude/worktrees/pane-move\nHEAD 3c4d\nbranch refs/heads/pane-move\n\n\
                   worktree /tmp/probe\nHEAD 5e6f\ndetached\nprunable gitdir file points to non-existent location\n\n";
        let all = parse_worktrees(out);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].branch.as_deref(), Some("main"));
        assert_eq!(
            all[1],
            Worktree {
                path: "/src/gerdr/.claude/worktrees/pane-move".into(),
                head: Some("3c4d".into()),
                branch: Some("pane-move".into()),
                ..Worktree::default()
            }
        );
        assert_eq!(all[2].branch, None);
        assert!(all[2].gone);
        assert_eq!(all[1].name(), "pane-move");
        assert_eq!(all[2].name(), "probe (detached)");
        assert_eq!(all[2].attention_tag(), Some("gone"));
        assert_eq!(all[1].attention_tag(), Some("unmerged"));
        let done = Worktree {
            merged: true,
            ..all[1].clone()
        };
        assert!(!done.needs_attention());
        assert!(
            Worktree {
                dirty: true,
                ..done
            }
            .needs_attention()
        );
    }

    #[test]
    fn real_changes_round_trip() {
        let repo = Repo::new();
        let dir = &repo.0;
        repo.commit(dir, "b", "b");
        repo.commit(dir, "c", "c");
        repo.git(dir, &["mv", "a", "a renamed"]);
        std::fs::remove_file(dir.join("b")).unwrap();
        std::fs::write(dir.join("c"), "changed").unwrap();
        std::fs::create_dir(dir.join("new dir")).unwrap();
        std::fs::write(dir.join("new dir/x"), "x").unwrap();
        std::fs::write(dir.join("tab\there\nand ünï"), "x").unwrap();
        let status = status(dir).unwrap();
        assert!(status.dirty);
        assert_eq!(
            status.changes,
            vec![
                Change {
                    orig: Some("a".into()),
                    ..change('R', "a renamed")
                },
                change('D', "b"),
                change('M', "c"),
                change('?', "new dir/"),
                change('?', "tab\there\nand ünï"),
            ]
        );
        assert!(status.head.is_some());
    }

    #[test]
    fn a_repository_without_commits() {
        let repo = Repo::new();
        let empty = repo.beside("empty");
        std::fs::create_dir(&empty).unwrap();
        repo.git(&empty, &["init", "-q", "-b", "main"]);
        let status = status(&empty).unwrap();
        assert_eq!(status.head, None);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.root.as_ref(), Some(&empty));
        assert_eq!(status.git_dir, Some(empty.join(".git")));
        assert_eq!(status.common_dir, status.git_dir);
        // main is only named yet, not made.
        assert_eq!(status.default_branch, None);
        assert_eq!(
            (status.remote, status.op, status.fetched),
            (None, None, None)
        );
        assert!(!status.dirty && status.changes.is_empty());
    }

    #[test]
    fn a_merge_stopped_on_a_conflict() {
        let repo = Repo::new();
        repo.branch_commit("topic", "f", "topic");
        repo.commit(&repo.0, "f", "main");
        assert_eq!(status(&repo.0).unwrap().op, None);
        assert!(!repo.try_git(&repo.0, &["merge", "-q", "topic"]));
        let stopped = status(&repo.0).unwrap();
        assert_eq!(stopped.op, Some(InProgress::Merge));
        assert_eq!(stopped.changes, vec![change('U', "f")]);
        assert!(stopped.dirty);
        repo.git(&repo.0, &["merge", "--abort"]);
        assert_eq!(status(&repo.0).unwrap().op, None);
    }

    #[test]
    fn a_rebase_stopped_on_a_conflict() {
        let repo = Repo::new();
        repo.git(&repo.0, &["switch", "-q", "-c", "topic"]);
        repo.commit(&repo.0, "x", "one");
        repo.commit(&repo.0, "f", "two");
        repo.commit(&repo.0, "y", "three");
        repo.git(&repo.0, &["switch", "-q", "main"]);
        repo.commit(&repo.0, "f", "main");
        // The merge backend keeps its place in rebase-merge/, the apply one
        // in rebase-apply/.
        for backend in ["--merge", "--apply"] {
            let rebase = ["rebase", "-q", backend, "main", "topic"];
            assert!(!repo.try_git(&repo.0, &rebase), "{backend}");
            let stopped = status(&repo.0).unwrap();
            assert_eq!(
                stopped.op,
                Some(InProgress::Rebase { step: 2, total: 3 }),
                "{backend}"
            );
            assert_eq!(stopped.branch, None);
            assert!(stopped.changes.iter().any(Change::conflicted), "{backend}");
            repo.git(&repo.0, &["rebase", "--abort"]);
            assert_eq!(status(&repo.0).unwrap().op, None);
        }
    }

    #[test]
    fn a_cherry_pick_or_revert_stopped_on_a_conflict() {
        let repo = Repo::new();
        repo.branch_commit("topic", "f", "topic");
        repo.commit(&repo.0, "f", "main");
        assert!(!repo.try_git(&repo.0, &["cherry-pick", "topic"]));
        let stopped = status(&repo.0).unwrap();
        assert_eq!(stopped.op, Some(InProgress::CherryPick));
        assert_eq!(stopped.changes, vec![change('U', "f")]);
        repo.git(&repo.0, &["cherry-pick", "--abort"]);
        // Undoing the commit that made f, once f has changed since.
        repo.commit(&repo.0, "f", "again");
        assert!(!repo.try_git(&repo.0, &["revert", "--no-edit", "HEAD~1"]));
        assert_eq!(status(&repo.0).unwrap().op, Some(InProgress::Revert));
    }

    #[test]
    fn stashes_are_counted_in_every_worktree() {
        let repo = Repo::new();
        let wt = repo.worktree("wt");
        assert_eq!(status(&repo.0).unwrap().stashes, 0);
        for text in ["1", "2"] {
            std::fs::write(repo.0.join("a"), text).unwrap();
            repo.git(&repo.0, &["stash", "push", "-q"]);
        }
        assert_eq!(status(&repo.0).unwrap().stashes, 2);
        assert_eq!(status(&wt).unwrap().stashes, 2);
    }

    #[test]
    fn a_linked_worktree_shares_the_git_folder() {
        let repo = Repo::new();
        let wt = repo.worktree("wt");
        std::fs::create_dir(wt.join("sub")).unwrap();
        let main = status(&repo.0).unwrap();
        let linked = status(&wt.join("sub")).unwrap();
        let common = repo.0.join(".git");
        assert_eq!(main.git_dir.as_ref(), Some(&common));
        assert_eq!(main.common_dir.as_ref(), Some(&common));
        assert_eq!(linked.git_dir, Some(common.join("worktrees/wt")));
        assert_eq!(linked.common_dir.as_ref(), Some(&common));
        assert_eq!(linked.root.as_ref(), Some(&wt));
        // From a subfolder git prints the shared folder as "../.git", and
        // the subfolder may be reached through a symlink.
        std::fs::create_dir(repo.0.join("sub")).unwrap();
        let link = repo.beside("link");
        std::os::unix::fs::symlink(repo.0.join("sub"), &link).unwrap();
        assert_eq!(status(&link).unwrap().common_dir, Some(common));
    }

    #[test]
    fn upstream_remote_and_default_branch() {
        let repo = Repo::new();
        let before = status(&repo.0).unwrap();
        assert_eq!((before.remote, before.upstream), (None, None));
        assert_eq!(before.fetched, None);
        let remote = repo.bare_remote();
        // The remote's default branch is trunk, which a local main doesn't
        // override.
        repo.git(&repo.0, &["push", "-q", "origin", "main:trunk"]);
        repo.git(&remote, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
        repo.git(&repo.0, &["fetch", "-q", "origin"]);
        repo.git(&repo.0, &["remote", "set-head", "origin", "-a"]);
        let fetched = status(&repo.0).unwrap();
        assert_eq!(fetched.remote.as_deref(), Some("origin"));
        assert_eq!(fetched.upstream, None);
        assert_eq!(fetched.default_branch.as_deref(), Some("trunk"));
        assert!(fetched.fetched.is_some());
        repo.git(&repo.0, &["branch", "-q", "-u", "origin/main"]);
        repo.commit(&repo.0, "b", "ahead");
        let ahead = status(&repo.0).unwrap();
        assert_eq!(ahead.upstream.as_deref(), Some("origin/main"));
        assert_eq!((ahead.ahead, ahead.behind), (1, 0));
        // Packed, the branches are still found.
        repo.git(&repo.0, &["pack-refs", "--all"]);
        let packed = status(&repo.0).unwrap();
        assert_eq!(packed.default_branch.as_deref(), Some("trunk"));
        assert_eq!(packed.upstream.as_deref(), Some("origin/main"));
        // A remote of another name, the only one, and the upstream's.
        repo.git(&repo.0, &["remote", "rename", "origin", "fork"]);
        let renamed = status(&repo.0).unwrap();
        assert_eq!(renamed.remote.as_deref(), Some("fork"));
        assert_eq!(renamed.upstream.as_deref(), Some("fork/main"));
        assert_eq!(renamed.default_branch.as_deref(), Some("trunk"));
    }

    #[test]
    fn local_and_remote_branches() {
        let repo = Repo::new();
        let alone = status(&repo.0).unwrap();
        assert_eq!(alone.local_branches, ["main"]);
        assert!(alone.remote_branches.is_empty());
        repo.bare_remote();
        repo.git(
            &repo.0,
            &["push", "-q", "origin", "main:main", "main:feat/x"],
        );
        repo.git(&repo.0, &["fetch", "-q", "origin"]);
        repo.git(&repo.0, &["remote", "set-head", "origin", "-a"]);
        repo.git(&repo.0, &["branch", "-q", "work"]);
        // Another remote's branches, and one whose name starts the same,
        // aren't the remote's.
        let other = repo.beside("other");
        repo.git(&repo.0, &["init", "-q", "--bare", other.to_str().unwrap()]);
        for name in ["upstream", "origin-x"] {
            repo.git(&repo.0, &["remote", "add", name, other.to_str().unwrap()]);
            repo.git(&repo.0, &["push", "-q", name, "main:other"]);
            repo.git(&repo.0, &["fetch", "-q", name]);
        }
        let both = status(&repo.0).unwrap();
        assert_eq!(both.remote.as_deref(), Some("origin"));
        assert_eq!(both.local_branches, ["main", "work"]);
        assert_eq!(both.remote_branches, ["feat/x", "main"]);
        // Each has the time of its last commit.
        let mut timed: Vec<&str> = both.branch_times.keys().map(String::as_str).collect();
        timed.sort_unstable();
        assert_eq!(timed, ["feat/x", "main", "work"]);
        assert!(both.branch_times.values().all(|&t| t > 0));
    }

    #[test]
    fn a_fetch_in_any_worktree_counts() {
        let repo = Repo::new();
        let wt = repo.worktree("wt");
        repo.bare_remote();
        assert_eq!(status(&repo.0).unwrap().fetched, None);
        repo.git(&wt, &["fetch", "-q", "origin"]);
        // Git keeps it with the worktree that fetched.
        assert!(!repo.0.join(".git/FETCH_HEAD").exists());
        assert!(repo.0.join(".git/worktrees/wt/FETCH_HEAD").exists());
        assert!(status(&repo.0).unwrap().fetched.is_some());
        assert!(status(&wt).unwrap().fetched.is_some());
    }

    #[test]
    fn default_branch_without_a_remote_head() {
        let repo = Repo::new();
        let default = |dir: &Path| status(dir).unwrap().default_branch;
        assert_eq!(default(&repo.0).as_deref(), Some("main"));
        repo.git(&repo.0, &["pack-refs", "--all"]);
        assert!(!repo.0.join(".git/refs/heads/main").exists());
        assert_eq!(default(&repo.0).as_deref(), Some("main"));
        repo.git(&repo.0, &["branch", "-q", "-m", "main", "master"]);
        assert_eq!(default(&repo.0).as_deref(), Some("master"));
        repo.git(&repo.0, &["branch", "-q", "-m", "master", "trunk"]);
        assert_eq!(default(&repo.0), None);
        // A branch under main/ isn't main.
        repo.git(&repo.0, &["branch", "-q", "main/x"]);
        assert_eq!(default(&repo.0), None);
        // A reftable store has no files to read: git answers.
        let table = repo.beside("table");
        std::fs::create_dir(&table).unwrap();
        repo.git(
            &table,
            &["init", "-q", "-b", "work", "--ref-format=reftable"],
        );
        repo.commit(&table, "a", "first");
        repo.git(
            &table,
            &["remote", "add", "origin", repo.0.to_str().unwrap()],
        );
        repo.git(&repo.0, &["branch", "-q", "-D", "main/x"]);
        repo.git(&repo.0, &["branch", "-q", "-m", "trunk", "main"]);
        repo.git(&table, &["fetch", "-q", "origin"]);
        repo.git(&table, &["remote", "set-head", "origin", "-a"]);
        assert!(table.join(".git/reftable").is_dir());
        assert_eq!(default(&table).as_deref(), Some("main"));
    }

    /// Every git of gerdr's runs without optional locks and with no
    /// `GIT_DIR` of a caller (a git hook's, say) pointing it elsewhere.
    #[test]
    fn command_sets_its_env() {
        let cmd = super::command(Path::new("/"), &["status"]);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.contains(&("GIT_OPTIONAL_LOCKS".as_ref(), Some("0".as_ref()))));
        assert!(envs.contains(&("GIT_DIR".as_ref(), None)));
    }
}
