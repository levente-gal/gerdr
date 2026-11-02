//! A feature's folder of worktrees beside the project's repositories, and
//! finding those repositories.
//!
//! Every function here runs git and touches the disk: call it off the UI
//! thread (checklist D3) and hand the result back over a channel.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::{Feature, Repo, hook};

/// The folder features' worktrees go in, under a root.
pub const FEATURES_DIR: &str = ".features";

/// Where a project's repositories are, as a new feature would use them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// The repository's top folder for a single one, else the folder
    /// holding several.
    pub path: PathBuf,
    pub multi: bool,
    /// Its repositories in name order: only main worktrees count (a
    /// secondary worktree such as `services-api-perf` is not one).
    pub repos: Vec<Found>,
}

/// A repository found under a root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Its folder's name.
    pub name: String,
    /// Its main worktree.
    pub main: PathBuf,
    /// The branch features start from: `override` if one is set, else
    /// `origin/HEAD`'s branch, else the main worktree's current branch.
    pub base: String,
}

/// The root a feature started from `dir` would use: `dir`'s repository
/// alone, or, when the project has workspaces in two or more repositories
/// with the same parent folder (`workspace_dirs`), that parent with every
/// repository in it. Repositories named in `ignore` are left out; `bases`
/// overrides the base branch by repository name. An error says why there
/// is none ("~ is not in a git repository").
pub fn find_root(
    dir: &Path,
    workspace_dirs: &[PathBuf],
    ignore: &[String],
    bases: &[(String, String)],
) -> Result<Root, String> {
    if let Some(top) = main_top(dir) {
        let parent = top.parent().map(Path::to_path_buf);
        let mut siblings: Vec<PathBuf> = workspace_dirs
            .iter()
            .filter_map(|d| main_top(d))
            .filter(|t| parent.is_some() && t.parent() == parent.as_deref())
            .collect();
        siblings.sort();
        siblings.dedup();
        if siblings.len() >= 2
            && let Some(parent) = parent
        {
            return multi_root(&parent, ignore, bases);
        }
        let name = folder_name(&top);
        let base = base_of(&name, &top, bases)?;
        return Ok(Root {
            path: top.clone(),
            multi: false,
            repos: vec![Found {
                name,
                main: top,
                base,
            }],
        });
    }
    if !child_repos(dir).is_empty() {
        return multi_root(dir, ignore, bases);
    }
    Err(format!(
        "{} is not in a git repository and has none in it",
        shown(dir)
    ))
}

/// The root `dir` holding several repositories.
fn multi_root(dir: &Path, ignore: &[String], bases: &[(String, String)]) -> Result<Root, String> {
    let path = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut repos = Vec::new();
    for (name, main) in child_repos(&path) {
        if ignore.contains(&name) {
            continue;
        }
        let base = base_of(&name, &main, bases)?;
        repos.push(Found { name, main, base });
    }
    if repos.is_empty() {
        return Err(format!(
            "{} has no git repositories left once the ignored ones are out",
            shown(&path)
        ));
    }
    Ok(Root {
        path,
        multi: true,
        repos,
    })
}

/// The repositories right under `dir`, by name: folders whose `.git` is a
/// folder (main worktrees; a `.git` file is a linked worktree's).
fn child_repos(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut repos: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let path = e.path();
            (name != FEATURES_DIR && path.join(".git").is_dir()).then_some((name, path))
        })
        .collect();
    repos.sort();
    repos
}

/// The top folder of the main worktree of the repository `dir` is in.
fn main_top(dir: &Path) -> Option<PathBuf> {
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let common = PathBuf::from(common.trim());
    let top = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent()?.to_path_buf()
    } else {
        // A submodule or a git folder kept elsewhere: its own top.
        PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"]).ok()?.trim())
    };
    Some(top.canonicalize().unwrap_or(top))
}

/// The branch features of repository `name` at `main` start from.
fn base_of(name: &str, main: &Path, bases: &[(String, String)]) -> Result<String, String> {
    if let Some((_, base)) = bases.iter().find(|(n, _)| n == name) {
        return Ok(base.clone());
    }
    if let Ok(head) = git(
        main,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        let head = head.trim();
        let head = head.strip_prefix("origin/").unwrap_or(head);
        if !head.is_empty() {
            return Ok(head.to_owned());
        }
    }
    if let Ok(branch) = git(main, &["symbolic-ref", "--short", "HEAD"]) {
        let branch = branch.trim();
        if !branch.is_empty() {
            return Ok(branch.to_owned());
        }
    }
    Err(format!(
        "{name} has no branch to start features from: it has no origin/HEAD and is on no \
         branch; set its base in the project's settings"
    ))
}

/// What `create` did to files outside the feature's folder, to tell the
/// user (checklist D1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Created {
    pub repos: Vec<Repo>,
    /// `<root>/.features/<slug>`.
    pub dir: PathBuf,
    /// The ignore files `.features/` was added to (`.git/info/exclude`,
    /// `.cursorignore`, `.graphifyignore`, `pyrightconfig.json`): only
    /// those that changed.
    pub ignores: Vec<PathBuf>,
    /// The root's files and folders other than repositories (AGENTS.md,
    /// .agents) linked into the feature folder, multi-repository only.
    pub links: Vec<PathBuf>,
}

/// Make feature `slug`'s folder under `root`: a worktree per repository,
/// on branch `feature/<slug>` from its base for those named in `touched`,
/// detached at its base for the rest. For a single repository the
/// worktree is the folder itself. Keeps `.features/` out of git and the
/// tools that index the root. On a failure, what was made is taken down
/// again before the error returns.
pub fn create(root: &Root, slug: &str, touched: &[String]) -> Result<Created, String> {
    if slug.is_empty() || slug.starts_with('.') || slug.contains(['/', '\\']) {
        return Err(format!("\"{slug}\" can't name a feature's folder"));
    }
    if root.repos.is_empty() {
        return Err(format!("{} has no repositories", shown(&root.path)));
    }
    if let Some(name) = touched
        .iter()
        .find(|t| !root.repos.iter().any(|r| &r.name == *t))
    {
        return Err(format!("{} has no repository {name}", shown(&root.path)));
    }
    let features = root.path.join(FEATURES_DIR);
    let dir = features.join(slug);
    if dir.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", shown(&dir)));
    }
    let branch = format!("feature/{slug}");
    for repo in &root.repos {
        if branch_exists(&repo.main, &branch) {
            return Err(format!("{} already has a branch {branch}", repo.name));
        }
    }
    let mut made = Made {
        features: (!features.exists()).then(|| features.clone()),
        dir: dir.clone(),
        worktrees: Vec::new(),
    };
    match build(root, &dir, &branch, touched, &mut made) {
        Ok(created) => Ok(created),
        Err(err) => {
            made.undo();
            Err(err)
        }
    }
}

/// What `create` has made so far, to take down on a failure.
struct Made {
    /// `.features`, when `create` made it.
    features: Option<PathBuf>,
    dir: PathBuf,
    /// Each worktree's main worktree, folder, and branch made for it.
    worktrees: Vec<(PathBuf, PathBuf, Option<String>)>,
}

impl Made {
    fn undo(self) {
        for (main, path, branch) in self.worktrees.iter().rev() {
            let p = path.to_string_lossy();
            if let Err(err) = git(main, &["worktree", "remove", "--force", &p]) {
                tracing::warn!("taking down {}: {err}", path.display());
            }
            if let Some(branch) = branch
                && let Err(err) = git(main, &["branch", "-D", branch])
            {
                tracing::warn!("deleting {branch} in {}: {err}", main.display());
            }
        }
        if let Err(err) = std::fs::remove_dir_all(&self.dir)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("removing {}: {err}", self.dir.display());
        }
        for (main, _, _) in &self.worktrees {
            if let Err(err) = git(main, &["worktree", "prune"]) {
                tracing::warn!("pruning worktrees of {}: {err}", main.display());
            }
        }
        if let Some(features) = self.features {
            // Only when empty: a failure means something else is in it.
            let _ = std::fs::remove_dir(features);
        }
    }
}

/// `create`'s work, noting in `made` what it makes.
fn build(
    root: &Root,
    dir: &Path,
    branch: &str,
    touched: &[String],
    made: &mut Made,
) -> Result<Created, String> {
    if root.multi {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("couldn't make {}: {err}", shown(dir)))?;
    } else if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("couldn't make {}: {err}", shown(parent)))?;
    }
    let mut repos = Vec::new();
    for found in &root.repos {
        let is_touched = !root.multi || touched.contains(&found.name);
        let path = if root.multi {
            dir.join(&found.name)
        } else {
            dir.to_path_buf()
        };
        let commit =
            commit_of(&found.main, &found.base).map_err(|err| format!("{}: {err}", found.name))?;
        let p = path.to_string_lossy();
        let args: Vec<&str> = if is_touched {
            vec!["worktree", "add", "-q", "-b", branch, &p, &commit]
        } else {
            vec!["worktree", "add", "-q", "--detach", &p, &commit]
        };
        git(&found.main, &args).map_err(|err| format!("{}: {err}", found.name))?;
        made.worktrees.push((
            found.main.clone(),
            path.clone(),
            is_touched.then(|| branch.to_owned()),
        ));
        repos.push(Repo {
            name: found.name.clone(),
            main: found.main.clone(),
            base: found.base.clone(),
            base_commit: commit,
            worktree: path,
            touched: is_touched,
            branch: is_touched.then(|| branch.to_owned()),
        });
    }
    let links = if root.multi {
        link_root(&root.path, dir)?
    } else {
        Vec::new()
    };
    let ignores = keep_out(root)?;
    for repo in repos.iter().filter(|r| r.touched) {
        guard_worktree(repo, branch);
    }
    Ok(Created {
        repos,
        dir: dir.to_path_buf(),
        ignores,
        links,
    })
}

/// Whether `main`'s repository has a local branch `branch`.
fn branch_exists(main: &Path, branch: &str) -> bool {
    let full = format!("refs/heads/{branch}");
    git(main, &["rev-parse", "--verify", "--quiet", &full]).is_ok()
}

/// The commit `base` is at in `main`'s repository: the local branch, else
/// origin's.
fn commit_of(main: &Path, base: &str) -> Result<String, String> {
    for name in [base.to_owned(), format!("origin/{base}")] {
        let spec = format!("{name}^{{commit}}");
        if let Ok(oid) = git(main, &["rev-parse", "--verify", "--quiet", &spec]) {
            return Ok(oid.trim().to_owned());
        }
    }
    Err(format!("its base branch {base} isn't there"))
}

/// The ref `losses` and `remove` count commits from for `repo`: its base
/// branch if it is there, else origin's, else the commit it started at.
pub(super) fn base_ref(repo: &Repo) -> String {
    for name in [repo.base.clone(), format!("origin/{}", repo.base)] {
        let spec = format!("{name}^{{commit}}");
        if git(&repo.main, &["rev-parse", "--verify", "--quiet", &spec]).is_ok() {
            return name;
        }
    }
    repo.base_commit.clone()
}

/// Root entries a multi-repository feature folder links to: not
/// repositories or their worktrees, and not these.
const UNLINKED: [&str; 4] = [FEATURES_DIR, ".git", "graphify-out", "node_modules"];

/// Link the root's files and folders other than repositories into `dir`.
fn link_root(root: &Path, dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(root).map_err(|err| format!("couldn't list {}: {err}", shown(root)))?;
    let mut names: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .filter(|name| {
            let path = root.join(name);
            !UNLINKED.iter().any(|u| name == *u) && path.join(".git").symlink_metadata().is_err()
        })
        .collect();
    names.sort();
    let mut links = Vec::new();
    for name in names {
        let link = dir.join(&name);
        std::os::unix::fs::symlink(root.join(&name), &link)
            .map_err(|err| format!("couldn't link {}: {err}", shown(&link)))?;
        links.push(link);
    }
    Ok(links)
}

/// Keep `.features/` out of git (single repository) and the tools that
/// index the root; the files that changed.
fn keep_out(root: &Root) -> Result<Vec<PathBuf>, String> {
    let mut changed = Vec::new();
    if !root.multi {
        let common = git(
            &root.path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let exclude = PathBuf::from(common.trim()).join("info").join("exclude");
        let text = match std::fs::read_to_string(&exclude) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(format!("couldn't read {}: {err}", shown(&exclude))),
        };
        if let Some(text) = add_line(&text, "/.features/") {
            if let Some(info) = exclude.parent() {
                std::fs::create_dir_all(info)
                    .map_err(|err| format!("couldn't make {}: {err}", shown(info)))?;
            }
            std::fs::write(&exclude, text)
                .map_err(|err| format!("couldn't write {}: {err}", shown(&exclude)))?;
            changed.push(exclude);
        }
    }
    // The tools' files are only edited when there; one that can't be is
    // logged and left, as a feature can do without it.
    for name in [".cursorignore", ".graphifyignore", "pyrightconfig.json"] {
        let path = root.path.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let new = if name == "pyrightconfig.json" {
            match add_pyright_exclude(&text) {
                Ok(new) => new,
                Err(err) => {
                    tracing::warn!("leaving {}: {err}", path.display());
                    continue;
                }
            }
        } else {
            add_line(&text, ".features/")
        };
        let Some(new) = new else { continue };
        match std::fs::write(&path, new) {
            Ok(()) => changed.push(path),
            Err(err) => tracing::warn!("writing {}: {err}", path.display()),
        }
    }
    Ok(changed)
}

/// `text` with `line` added, unless a line already ignores `.features`.
fn add_line(text: &str, line: &str) -> Option<String> {
    let has = text.lines().any(|l| {
        matches!(
            l.trim(),
            ".features"
                | ".features/"
                | "/.features"
                | "/.features/"
                | "**/.features"
                | "**/.features/"
        )
    });
    if has {
        return None;
    }
    let mut text = text.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    Some(text)
}

/// `text`, a pyrightconfig.json, with `.features` in its `exclude`, unless
/// it is there already or there is no `exclude`: pyright's own excludes
/// (used when a config names none) leave out every dot folder already.
/// Edited as text, so the file's comments, trailing commas and layout
/// (pyright reads it as JSONC) stay as they are.
fn add_pyright_exclude(text: &str) -> Result<Option<String>, String> {
    let mut from = 0;
    while let Some(at) = text[from..].find("\"exclude\"") {
        let key_end = from + at + "\"exclude\"".len();
        from = key_end;
        let rest = &text[key_end..];
        let Some(rest) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(list) = rest.trim_start().strip_prefix('[') else {
            return Err("its exclude isn't a list".into());
        };
        let open = text.len() - list.len();
        let Some(close) = list.find(']') else {
            return Err("its exclude list doesn't end".into());
        };
        let items = &list[..close];
        let has = items.split(',').any(|item| {
            let item = item.trim().trim_matches('"');
            let item = item.trim_start_matches("./").trim_end_matches('/');
            matches!(item, ".features" | "**/.features")
        });
        if has {
            return Ok(None);
        }
        let entry = if items.trim().is_empty() {
            "\".features\""
        } else {
            "\".features\", "
        };
        let mut new = text.to_owned();
        new.insert_str(open, entry);
        return Ok(Some(new));
    }
    Ok(None)
}

/// Put `repo`'s worktree of `feature` on branch `feature/<slug>` at the
/// commit it is on, so the feature can change it. Returns the repository
/// as it is now.
pub fn touch(feature: &Feature, repo: &str) -> Result<Repo, String> {
    let Some(found) = feature.repos.iter().find(|r| r.name == repo) else {
        return Err(format!("{} has no repository {repo}", feature.title));
    };
    if found.touched {
        return Err(format!(
            "{} already has {repo} on its branch",
            feature.title
        ));
    }
    if !found.worktree.is_dir() {
        return Err(format!("{} isn't there", shown(&found.worktree)));
    }
    let branch = feature.branch();
    git(&found.worktree, &["switch", "-q", "-c", &branch])
        .map_err(|err| format!("{repo}: {err}"))?;
    let repo = Repo {
        touched: true,
        branch: Some(branch.clone()),
        ..found.clone()
    };
    guard_worktree(&repo, &branch);
    Ok(repo)
}

/// Guard `branch` of `repo` and make sure of the `pre-push` hook in its
/// worktree's hooks folder. Best effort: a feature can be made without
/// it, and `ship::probe` makes sure of both again; a failure is logged.
fn guard_worktree(repo: &Repo, branch: &str) {
    if let Err(err) = hook::guard(&repo.main, branch) {
        tracing::warn!("guarding {branch} of {}: {err}", repo.name);
    }
    if let hook::Guard::Off(why) = hook::ensure(&repo.worktree) {
        tracing::warn!(
            "pushes from {} aren't guarded: {why}",
            repo.worktree.display()
        );
    }
}

/// What taking `feature`'s worktrees down would lose, one line per
/// repository that loses something ("services-api: 2 changed files, 3
/// commits not in dev"). Empty when nothing is lost.
pub fn losses(feature: &Feature) -> Vec<String> {
    feature.repos.iter().filter_map(loss).collect()
}

/// What taking `repo`'s worktree down would lose, as `losses` says it.
fn loss(repo: &Repo) -> Option<String> {
    if !repo.worktree.is_dir() {
        return None;
    }
    let status = match git(
        &repo.worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    ) {
        Ok(status) => status,
        Err(err) => return Some(format!("{}: couldn't tell what it has ({err})", repo.name)),
    };
    let mut parts = Vec::new();
    let changed = status.lines().filter(|l| !l.trim().is_empty()).count();
    if changed > 0 {
        parts.push(format!("{changed} changed {}", plural(changed, "file")));
    }
    if repo.touched {
        let range = format!("{}..HEAD", base_ref(repo));
        match git(&repo.worktree, &["rev-list", "--count", &range]) {
            Ok(count) => {
                let n: usize = count.trim().parse().unwrap_or(0);
                if n > 0 {
                    parts.push(format!("{n} {} not in {}", plural(n, "commit"), repo.base));
                }
            }
            Err(err) => parts.push(format!("couldn't count its commits ({err})")),
        }
    }
    (!parts.is_empty()).then(|| format!("{}: {}", repo.name, parts.join(", ")))
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

/// Take `feature`'s worktrees and folder down, and delete its branches.
/// `force` takes them down even when that loses work (`losses`); without
/// it, a repository that would lose something is left and named in the
/// error. A branch with commits not in its base is kept unless `force`.
pub fn remove(feature: &Feature, force: bool) -> Result<(), String> {
    if !force {
        let lost = losses(feature);
        if !lost.is_empty() {
            return Err(format!(
                "{} would lose work, so it was left: {}",
                feature.title,
                lost.join("; ")
            ));
        }
    }
    let mut failed = Vec::new();
    let mut left = false;
    let branch = feature.branch();
    for repo in &feature.repos {
        if repo.worktree.symlink_metadata().is_ok() {
            let p = repo.worktree.to_string_lossy();
            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(&p);
            if let Err(err) = git(&repo.main, &args) {
                failed.push(format!("{}: {err}", repo.name));
                left = true;
                continue;
            }
        } else if let Err(err) = git(&repo.main, &["worktree", "prune"]) {
            failed.push(format!("{}: {err}", repo.name));
        }
        if !repo.touched {
            continue;
        }
        if let Err(err) = hook::unguard(&repo.main, &branch) {
            tracing::warn!("unguarding {branch} of {}: {err}", repo.name);
        }
        if !branch_exists(&repo.main, &branch) {
            continue;
        }
        if !force {
            let range = format!("{}..{branch}", base_ref(repo));
            let ahead = git(&repo.main, &["rev-list", "--count", &range])
                .ok()
                .and_then(|n| n.trim().parse::<usize>().ok());
            if ahead != Some(0) {
                failed.push(format!(
                    "{}: kept branch {branch}, it has commits not in {}",
                    repo.name, repo.base
                ));
                continue;
            }
        }
        if let Err(err) = git(&repo.main, &["branch", "-D", &branch]) {
            failed.push(format!("{}: {err}", repo.name));
        }
    }
    if !left
        && feature.dir.symlink_metadata().is_ok()
        && let Err(err) = clear_dir(&feature.dir, force)
    {
        failed.push(err);
    }
    if let Some(features) = feature.dir.parent()
        && features.file_name().is_some_and(|n| n == FEATURES_DIR)
    {
        // Only when empty: other features may be in it.
        let _ = std::fs::remove_dir(features);
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// Remove a feature folder whose worktrees are gone: its links, then the
/// folder; anything else in it only with `force`.
fn clear_dir(dir: &Path, force: bool) -> Result<(), String> {
    if force {
        return std::fs::remove_dir_all(dir)
            .map_err(|err| format!("couldn't remove {}: {err}", shown(dir)));
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            if entry.file_type().is_ok_and(|t| t.is_symlink()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    std::fs::remove_dir(dir).map_err(|err| {
        if err.kind() == std::io::ErrorKind::DirectoryNotEmpty {
            format!("{} still has files of its own, so it was left", shown(dir))
        } else {
            format!("couldn't remove {}: {err}", shown(dir))
        }
    })
}

/// `path` as the user reads it: `~/…` under the home folder.
pub(super) fn shown(path: &Path) -> String {
    crate::paths::tilde(path, crate::home_dir().as_deref())
}

/// A folder's last name.
fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// What `git -C dir args` prints, or why it failed: never prompting, with
/// no `GIT_DIR`-style variables of a caller in the way. Like
/// `git_ops::exec` but synchronous and short: features run it off the UI
/// thread already.
pub(super) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_bytes(dir, args, None).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// `git`, fed `input` on its stdin, printing bytes.
pub(super) fn git_bytes(
    dir: &Path,
    args: &[&str],
    input: Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
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
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("couldn't run git: {err}"))?;
    // Fed from a thread of its own, so a git printing while it reads can't
    // block on a full pipe.
    let feed = input.zip(child.stdin.take()).map(|(input, mut stdin)| {
        std::thread::spawn(move || {
            use std::io::Write;
            // A git that stops reading early says why itself.
            let _ = stdin.write_all(&input);
        })
    });
    let out = child
        .wait_with_output()
        .map_err(|err| format!("couldn't run git: {err}"))?;
    if let Some(feed) = feed {
        // A feeder that panicked fed less: git said so already.
        let _ = feed.join();
    }
    if out.status.success() {
        return Ok(out.stdout);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    // The last line says why; the ones before it are hints.
    let why = err
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("hint:"))
        .unwrap_or("")
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ");
    Err(if why.is_empty() {
        format!("git {} failed", args.first().unwrap_or(&""))
    } else {
        why.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::feature::{Status, VERSION};

    /// A folder of its own under the system's temporary folder, removed
    /// when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-feature-layout-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }

        /// A repository `name` in it on branch `branch`, with a commit.
        fn repo(&self, name: &str, branch: &str) -> PathBuf {
            let dir = self.0.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            run(&dir, &["init", "-q", "-b", branch]);
            commit(&dir, "a", "first");
            dir
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        git(dir, args).unwrap_or_else(|err| panic!("git {args:?}: {err}"))
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-q", "-m", text]);
    }

    fn feature_of(root: &Root, created: Created, slug: &str) -> Feature {
        Feature {
            version: VERSION,
            slug: slug.into(),
            title: slug.into(),
            project: "p".into(),
            created: 1,
            root: root.path.clone(),
            dir: created.dir,
            multi: root.multi,
            repos: created.repos,
            status: Status::Open,
            marks: Default::default(),
            shipping: Default::default(),
            history: Vec::new(),
        }
    }

    fn names(root: &Root) -> Vec<&str> {
        root.repos.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn a_single_repository_is_its_own_feature_folder() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        std::fs::create_dir(repo.join("src")).unwrap();
        let root = find_root(&repo.join("src"), std::slice::from_ref(&repo), &[], &[]).unwrap();
        assert!(!root.multi);
        assert_eq!(root.path, repo);
        assert_eq!(root.repos[0].base, "main");

        let created = create(&root, "login", &[]).unwrap();
        let dir = repo.join(".features/login");
        assert_eq!(created.dir, dir);
        assert!(dir.join(".git").is_file());
        let r = &created.repos[0];
        assert!(r.touched);
        assert_eq!(r.branch.as_deref(), Some("feature/login"));
        assert_eq!(r.worktree, dir);
        assert_eq!(r.base_commit, run(&repo, &["rev-parse", "main"]).trim());
        assert_eq!(
            run(&dir, &["branch", "--show-current"]).trim(),
            "feature/login"
        );
        let exclude = repo.join(".git/info/exclude");
        assert_eq!(created.ignores, vec![exclude.clone()]);
        assert!(
            std::fs::read_to_string(&exclude)
                .unwrap()
                .contains("/.features/\n")
        );
        // The main worktree doesn't see the feature folder.
        assert_eq!(run(&repo, &["status", "--porcelain"]), "");
        // Its branch is guarded, and pushes go through gerdr's hook.
        assert!(hook::guarded(&repo, "feature/login"));
        let pre_push = std::fs::read_to_string(repo.join(".git/hooks/pre-push")).unwrap();
        assert_eq!(pre_push, hook::HOOK);

        let f = feature_of(&root, created, "login");
        assert!(losses(&f).is_empty());
        std::fs::write(dir.join("new"), "x").unwrap();
        commit(&dir, "b", "second");
        commit(&dir, "c", "third");
        assert_eq!(
            losses(&f),
            vec!["app: 1 changed file, 2 commits not in main".to_owned()]
        );
        let err = remove(&f, false).unwrap_err();
        assert!(
            err.contains("app: 1 changed file, 2 commits not in main"),
            "{err}"
        );
        assert!(dir.is_dir());
        assert!(branch_exists(&repo, "feature/login"));

        remove(&f, true).unwrap();
        assert!(!hook::guarded(&repo, "feature/login"));
        assert!(!dir.exists());
        assert!(!repo.join(".features").exists());
        assert!(!branch_exists(&repo, "feature/login"));
        assert!(losses(&f).is_empty());
    }

    #[test]
    fn a_clean_feature_goes_without_force() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &[]).unwrap();
        let f = feature_of(&root, created, "x");
        remove(&f, false).unwrap();
        assert!(!repo.join(".features").exists());
        assert!(!branch_exists(&repo, "feature/x"));
        assert_eq!(
            run(&repo, &["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
    }

    #[test]
    fn origin_head_and_overrides_pick_the_base() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        run(&repo, &["branch", "dev"]);
        run(&repo, &["update-ref", "refs/remotes/origin/dev", "HEAD"]);
        run(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/dev",
            ],
        );
        assert_eq!(
            find_root(&repo, &[], &[], &[]).unwrap().repos[0].base,
            "dev"
        );
        let bases = [("app".to_owned(), "release".to_owned())];
        assert_eq!(
            find_root(&repo, &[], &[], &bases).unwrap().repos[0].base,
            "release"
        );
    }

    #[test]
    fn no_repository_is_an_error() {
        let t = Temp::new();
        let err = find_root(&t.0, &[], &[], &[]).unwrap_err();
        assert!(err.contains("is not in a git repository"), "{err}");
    }

    #[test]
    fn sibling_workspaces_make_a_multi_root() {
        let t = Temp::new();
        let api = t.repo("services-api", "dev");
        let web = t.repo("web", "main");
        t.repo("legacy", "main");
        // A linked worktree beside them is no repository of its own.
        let perf = t.0.join("services-api-perf");
        run(
            &api,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "perf",
                perf.to_str().unwrap(),
            ],
        );
        std::fs::write(t.0.join("AGENTS.md"), "x").unwrap();

        // One workspace alone: the repository by itself.
        let single = find_root(&perf, std::slice::from_ref(&perf), &[], &[]).unwrap();
        assert!(!single.multi);
        assert_eq!(single.path, api);

        // Workspaces in two repositories, one of them from a worktree.
        let root = find_root(&perf, &[perf.clone(), web.clone()], &["legacy".into()], &[]).unwrap();
        assert!(root.multi);
        assert_eq!(root.path, t.0);
        assert_eq!(names(&root), ["services-api", "web"]);
        assert_eq!(root.repos[0].base, "dev");
        assert_eq!(root.repos[0].main, api);

        // A folder holding repositories, not in one.
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        assert!(root.multi);
        assert_eq!(names(&root), ["legacy", "services-api", "web"]);

        let all: Vec<String> = ["legacy", "services-api", "web"].map(String::from).into();
        let err = find_root(&t.0, &[], &all, &[]).unwrap_err();
        assert!(err.contains("no git repositories"), "{err}");
    }

    #[test]
    fn multi_creates_touched_and_detached_worktrees_with_links() {
        let t = Temp::new();
        let api = t.repo("api", "main");
        let web = t.repo("web", "main");
        let perf = t.0.join("api-perf");
        run(
            &api,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "perf",
                perf.to_str().unwrap(),
            ],
        );
        std::fs::write(t.0.join("AGENTS.md"), "x").unwrap();
        std::fs::create_dir(t.0.join(".agents")).unwrap();
        std::fs::create_dir(t.0.join("node_modules")).unwrap();
        std::fs::create_dir(t.0.join("graphify-out")).unwrap();
        std::fs::write(t.0.join(".cursorignore"), "target").unwrap();
        std::fs::write(t.0.join("pyrightconfig.json"), r#"{"venv": ".venv"}"#).unwrap();

        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "auth", &["web".into()]).unwrap();
        let dir = t.0.join(".features/auth");
        let (a, w) = (&created.repos[0], &created.repos[1]);
        assert!(!a.touched && a.branch.is_none());
        assert_eq!(a.worktree, dir.join("api"));
        assert_eq!(run(&a.worktree, &["branch", "--show-current"]).trim(), "");
        assert_eq!(
            run(&a.worktree, &["rev-parse", "HEAD"]).trim(),
            a.base_commit
        );
        assert!(w.touched);
        assert_eq!(
            run(&w.worktree, &["branch", "--show-current"]).trim(),
            "feature/auth"
        );
        assert!(!branch_exists(&api, "feature/auth"));
        assert!(branch_exists(&web, "feature/auth"));

        let linked: Vec<_> = created
            .links
            .iter()
            .map(|l| l.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            linked,
            [
                ".agents",
                ".cursorignore",
                "AGENTS.md",
                "pyrightconfig.json"
            ]
        );
        assert_eq!(
            std::fs::read_link(dir.join("AGENTS.md")).unwrap(),
            t.0.join("AGENTS.md")
        );
        // pyright leaves dot folders out unless a config names excludes.
        assert_eq!(created.ignores, vec![t.0.join(".cursorignore")]);
        assert!(!t.0.join(".graphifyignore").exists());
        assert_eq!(
            std::fs::read_to_string(t.0.join(".cursorignore")).unwrap(),
            "target\n.features/\n"
        );
        assert_eq!(
            std::fs::read_to_string(t.0.join("pyrightconfig.json")).unwrap(),
            r#"{"venv": ".venv"}"#
        );

        // A second feature changes no ignore file again.
        let second = create(&root, "other", &[]).unwrap();
        assert!(second.ignores.is_empty());

        // touch puts the detached one on the branch.
        let f = feature_of(&root, created, "auth");
        assert!(touch(&f, "web").unwrap_err().contains("already"));
        assert!(
            touch(&f, "nope")
                .unwrap_err()
                .contains("no repository nope")
        );
        assert!(!hook::guarded(&api, "feature/auth"));
        let touched = touch(&f, "api").unwrap();
        assert!(touched.touched);
        assert!(hook::guarded(&api, "feature/auth"));
        assert_eq!(touched.branch.as_deref(), Some("feature/auth"));
        assert_eq!(
            run(&touched.worktree, &["branch", "--show-current"]).trim(),
            "feature/auth"
        );
        let mut f = f;
        f.repos[0] = touched;

        // Untracked in the detached... now touched api: a loss.
        std::fs::write(f.repos[0].worktree.join("n"), "x").unwrap();
        assert_eq!(losses(&f), vec!["api: 1 changed file".to_owned()]);
        assert!(remove(&f, false).is_err());
        assert!(dir.join("api").is_dir());
        std::fs::remove_file(f.repos[0].worktree.join("n")).unwrap();
        remove(&f, false).unwrap();
        assert!(!dir.exists());
        assert!(t.0.join("AGENTS.md").is_file());
        assert!(t.0.join(".agents").is_dir());
        assert!(!branch_exists(&api, "feature/auth"));
        assert!(!branch_exists(&web, "feature/auth"));
        // The other feature keeps .features.
        assert!(t.0.join(".features/other").is_dir());
    }

    #[test]
    fn pyright_excludes_are_edited_as_text() {
        let jsonc = "{\n  // dev\n  \"exclude\": [\n    \"build\",\n  ],\n}\n";
        assert_eq!(
            add_pyright_exclude(jsonc).unwrap().unwrap(),
            "{\n  // dev\n  \"exclude\": [\".features\", \n    \"build\",\n  ],\n}\n"
        );
        assert_eq!(
            add_pyright_exclude(r#"{"exclude": []}"#).unwrap().unwrap(),
            r#"{"exclude": [".features"]}"#
        );
        assert_eq!(
            add_pyright_exclude(r#"{"exclude": ["./.features/"]}"#),
            Ok(None)
        );
        assert_eq!(add_pyright_exclude(r#"{"venv": ".v",}"#), Ok(None));
        assert!(add_pyright_exclude(r#"{"exclude": "x"}"#).is_err());
    }

    #[test]
    fn ignore_files_are_edited_only_when_there_and_once() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        std::fs::write(repo.join(".graphifyignore"), "out/\n.features/\n").unwrap();
        std::fs::write(repo.join("pyrightconfig.json"), r#"{"exclude": ["build"]}"#).unwrap();
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let created = create(&root, "a", &[]).unwrap();
        assert_eq!(
            created.ignores,
            vec![
                repo.join(".git/info/exclude"),
                repo.join("pyrightconfig.json")
            ]
        );
        assert!(!repo.join(".cursorignore").exists());
        assert_eq!(
            std::fs::read_to_string(repo.join(".graphifyignore")).unwrap(),
            "out/\n.features/\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("pyrightconfig.json")).unwrap(),
            r#"{"exclude": [".features", "build"]}"#
        );
        let again = create(&root, "b", &[]).unwrap();
        assert!(again.ignores.is_empty(), "{:?}", again.ignores);
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches(".features").count(), 1);
    }

    #[test]
    fn a_taken_branch_or_folder_is_refused() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        run(&repo, &["branch", "feature/a"]);
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let err = create(&root, "a", &[]).unwrap_err();
        assert_eq!(err, "app already has a branch feature/a");
        assert!(!repo.join(".features").exists());
        std::fs::create_dir_all(repo.join(".features/b")).unwrap();
        assert!(
            create(&root, "b", &[])
                .unwrap_err()
                .contains("already exists")
        );
        assert!(create(&root, "a/b", &[]).is_err());
    }

    #[test]
    fn a_failure_takes_down_what_was_made() {
        let t = Temp::new();
        let api = t.repo("api", "main");
        let web = t.repo("web", "main");
        std::fs::write(t.0.join("AGENTS.md"), "x").unwrap();
        // web's base isn't there: api's worktree is made first, then undone.
        let bases = [("web".to_owned(), "nope".to_owned())];
        let root = find_root(&t.0, &[], &[], &bases).unwrap();
        let err = create(&root, "f", &["api".into(), "web".into()]).unwrap_err();
        assert!(err.contains("web") && err.contains("nope"), "{err}");
        assert!(!t.0.join(".features").exists());
        assert!(!branch_exists(&api, "feature/f"));
        assert!(!branch_exists(&web, "feature/f"));
        assert_eq!(
            run(&api, &["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
        assert!(
            create(&root, "f", &["zzz".into()])
                .unwrap_err()
                .contains("no repository zzz")
        );
    }

    #[test]
    fn remove_keeps_going_when_a_worktree_is_gone() {
        let t = Temp::new();
        t.repo("api", "main");
        let web = t.repo("web", "main");
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "g", &["web".into()]).unwrap();
        let f = feature_of(&root, created, "g");
        std::fs::remove_dir_all(&f.repos[1].worktree).unwrap();
        assert!(losses(&f).is_empty());
        remove(&f, false).unwrap();
        assert!(!t.0.join(".features").exists());
        assert!(!branch_exists(&web, "feature/g"));
        assert_eq!(
            run(&web, &["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
    }
}
