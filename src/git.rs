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
    Rebase { step: u32, total: u32 },
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
    /// Its branch is merged into the main worktree's HEAD, or merging it
    /// would change nothing there (it was squashed in): likely done with.
    pub merged: bool,
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

    /// Whether it still needs a look: changes in it, a branch not merged
    /// (or none), or its folder gone. A merged, clean one is done with.
    pub fn needs_attention(&self) -> bool {
        self.dirty || self.attention_tag().is_some()
    }

    /// Why it needs a look, in a word, besides changes (its dot).
    pub fn attention_tag(&self) -> Option<&'static str> {
        if self.gone {
            Some("gone")
        } else if !self.main && !self.merged {
            Some("unmerged")
        } else {
            None
        }
    }

    /// Where it stands, in a word or two: why it needs a look, else "main
    /// worktree" or "merged".
    pub fn tag(&self) -> &'static str {
        self.attention_tag()
            .unwrap_or(if self.main { "main worktree" } else { "merged" })
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
pub fn status(dir: &Path) -> Option<GitStatus> {
    let out = git(dir, &["status", "--porcelain=v2", "--branch"])?;
    let mut status = parse(&out);
    status.root = git(dir, &["rev-parse", "--show-toplevel"]).map(|t| PathBuf::from(t.trim()));
    (status.here, status.worktrees) = worktrees(dir, status.root.as_deref());
    Some(status)
}

/// What `git args` prints in `dir`, or what it says went wrong.
fn git_run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
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

/// What `git args` prints in `dir`; `None` when it fails or without git.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        // Leave index.lock alone, so a refresh never gets in the way of the
        // user's own git commands.
        .arg("--no-optional-locks")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The worktree of the repository `dir` is in at `here`, `dir`'s, and the
/// others; none when it has no others.
fn worktrees(dir: &Path, here: Option<&Path>) -> (Option<Worktree>, Vec<Worktree>) {
    let Some(list) = git(dir, &["worktree", "list", "--porcelain"]) else {
        return (None, Vec::new());
    };
    let mut all = parse_worktrees(&list);
    if all.len() < 2 {
        return (None, Vec::new());
    }
    // The first is the main worktree: what the others' branches merge into.
    all[0].main = true;
    let main_head = all[0].head.clone();
    let merged = main_head.as_deref().map_or_else(Vec::new, |head| {
        let out = git(
            dir,
            &["branch", "--format=%(refname:short)", "--merged", head],
        );
        out.unwrap_or_default().lines().map(str::to_owned).collect()
    });
    for w in &mut all {
        w.merged = !w.main
            && w.branch.as_ref().is_some_and(|b| {
                merged.contains(b) || main_head.as_deref().is_some_and(|m| squashed(dir, m, w))
            });
        w.dirty =
            !w.gone && git(&w.path, &["status", "--porcelain"]).is_some_and(|o| !o.is_empty());
    }
    let at = all.iter().position(|w| Some(w.path.as_path()) == here);
    (at.map(|i| all.remove(i)), all)
}

/// Whether merging `w`'s branch into `main_head` would change nothing:
/// it is in already, squashed or picked. What `git merge-tree` says is
/// kept per pair of commits, as those don't change.
fn squashed(dir: &Path, main_head: &str, w: &Worktree) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<(String, String), bool>>> = OnceLock::new();
    let Some(head) = w.head.clone() else {
        return false;
    };
    let key = (main_head.to_owned(), head);
    let seen = SEEN.get_or_init(Default::default);
    if let Some(&known) = seen.lock().unwrap().get(&key) {
        return known;
    }
    let merge = git(dir, &["merge-tree", "--write-tree", &key.0, &key.1]);
    let tree = git(dir, &["rev-parse", &format!("{}^{{tree}}", key.0)]);
    let squashed = match (merge, tree) {
        (Some(merge), Some(tree)) => merge.lines().next() == tree.lines().next(),
        _ => false,
    };
    seen.lock().unwrap().insert(key, squashed);
    squashed
}

/// Remove worktree `w` of the repository whose main worktree is at `main`,
/// and its branch once merged, or anyway when `force`: its changes, and its
/// commits not merged, are lost then. A gone one is pruned.
pub fn remove_worktree(main: &Path, w: &Worktree, force: bool) -> Result<(), String> {
    if w.main {
        return Err("the main worktree stays".into());
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
        && (w.merged || force)
    {
        // -D: -d takes a squashed branch for unmerged.
        git_run(main, &["branch", "-D", branch])?;
    }
    Ok(())
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
    if w.dirty {
        return Err(format!("{name} has uncommitted changes"));
    }
    let status = git_run(main, &["status", "--porcelain=v2", "--branch"])?;
    let main_status = parse(&status);
    let Some(target) = main_status.branch else {
        return Err("The main worktree has no branch checked out".into());
    };
    if main_status.dirty {
        return Err(format!("{target} has uncommitted changes"));
    }
    Ok(())
}

/// Read the status of each of `dirs` on a thread of its own, and hand those
/// in a repository to `send`.
pub fn read_all(
    dirs: Vec<PathBuf>,
    send: impl FnOnce(HashMap<PathBuf, GitStatus>) + Send + 'static,
) {
    std::thread::spawn(move || {
        send(
            dirs.into_iter()
                .filter_map(|dir| Some((status(&dir)?, dir)))
                .map(|(status, dir)| (dir, status))
                .collect(),
        )
    });
}

/// The web page of the repository `dir` is in: its `origin` remote's, as
/// an https address; `None` without one or without git.
pub fn web_url(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["remote", "get-url", "origin"])
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

/// The status in `git status --porcelain=v2 --branch` output: "# " header
/// lines about the branch, then a line per changed or untracked file.
fn parse(out: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in out.lines() {
        let Some(header) = line.strip_prefix("# ") else {
            status.dirty |= !line.is_empty();
            continue;
        };
        if let Some(head) = header.strip_prefix("branch.head ") {
            status.branch = (head != "(detached)").then(|| head.to_string());
        } else if let Some(counts) = header.strip_prefix("branch.ab ") {
            for count in counts.split_whitespace() {
                if let Some(n) = count.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = count.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::{
        GitStatus, Worktree, parse, parse_worktrees, remote_to_web, remove_worktree, squash_check,
        status,
    };
    use std::{
        path::{Path, PathBuf},
        process::Command,
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
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(ok.status.success(), "git {args:?}: {ok:?}");
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
        assert!(wip.dirty && !wip.merged);
        assert!(squash_check(&repo.0, &wip).is_err());
        assert!(remove_worktree(&repo.0, &wip, false).is_err());
        assert!(path.exists());
        assert_eq!(remove_worktree(&repo.0, &wip, true), Ok(()));
        assert!(!path.exists());
        assert!(remove_worktree(&repo.0, &main, true).is_err());
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

    #[test]
    fn clean_branch_with_upstream() {
        let out = "# branch.oid 1a2b\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n";
        assert_eq!(
            parse(out),
            GitStatus {
                branch: Some("main".into()),
                dirty: false,
                ahead: 2,
                behind: 1,
                worktrees: Vec::new(),
                here: None,
                root: None,
                ..GitStatus::default()
            }
        );
    }

    #[test]
    fn changes_make_it_dirty() {
        let out = "# branch.oid 1a2b\n# branch.head topic\n1 .M N... 100644 100644 100644 a b src/main.rs\n";
        assert_eq!(
            parse(out),
            GitStatus {
                branch: Some("topic".into()),
                dirty: true,
                ahead: 0,
                behind: 0,
                worktrees: Vec::new(),
                here: None,
                root: None,
                ..GitStatus::default()
            }
        );
        assert!(parse("# branch.head main\n? notes.txt\n").dirty);
    }

    #[test]
    fn detached_head_and_new_repo() {
        assert_eq!(
            parse("# branch.oid 1a2b\n# branch.head (detached)\n").branch,
            None
        );
        assert_eq!(
            parse("# branch.oid (initial)\n# branch.head master\n")
                .branch
                .as_deref(),
            Some("master")
        );
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
}
