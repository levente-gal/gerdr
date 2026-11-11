//! A feature's folder of worktrees beside the project's repositories, and
//! finding those repositories.
//!
//! Every function here runs git and touches the disk: call it off the UI
//! thread (checklist D3) and hand the result back over a channel.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use super::{Feature, KeptHead, Link, Repo, hook, pr::PullRef};

/// The folder features' worktrees go in, under a root.
pub const FEATURES_DIR: &str = ".features";
/// Denies Cursor's agents what it lists, on top of keeping it from indexing.
pub const CURSOR_DENY: &str = ".cursorignore";
/// Keeps what it lists from Cursor's index only.
pub const CURSOR_INDEX: &str = ".cursorindexingignore";

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

/// The root a feature of this project would use: every repository of the
/// project, wherever it was started from. `dir` is where it was started,
/// and `workspace_dirs` the project's workspaces.
///
/// One repository alone is a root by itself. Otherwise the root is the
/// folder holding the most of them, with every repository in that folder
/// if two workspaces share it or a workspace is that folder; repositories
/// elsewhere join as they are. With none in any workspace, a workspace's
/// folder that has repositories right under it is the root. Repositories
/// named in `ignore` are left out of a root of several; `bases` overrides
/// the base branch by repository name. An error says why there is none
/// ("~ is not in a git repository").
pub fn find_root(
    dir: &Path,
    workspace_dirs: &[PathBuf],
    ignore: &[String],
    bases: &[(String, String)],
) -> Result<Root, String> {
    // A feature's folder counts as the folder it sits under: it holds
    // worktrees, not repositories.
    let dir = above_features(dir);
    let here = main_top(&dir);
    let mut tops = Vec::new();
    let mut folders = Vec::new();
    for d in std::iter::once(&dir).chain(workspace_dirs) {
        let d = above_features(d);
        match main_top(&d) {
            Some(top) => tops.push(top),
            None => folders.push(d.canonicalize().unwrap_or(d)),
        }
    }
    tops.sort();
    tops.dedup();
    let count = |p: &Path| tops.iter().filter(|t| t.parent() == Some(p)).count();
    let shared = |p: &Path| count(p) >= 2 || folders.iter().any(|f| f == p);
    let here_parent = here.as_deref().and_then(Path::parent);
    let home = tops
        .iter()
        .filter_map(|t| t.parent())
        .max_by_key(|p| (shared(p), count(p), Some(*p) == here_parent));
    let Some(home) = home else {
        return match folders.iter().find(|f| !child_repos(f).is_empty()) {
            Some(folder) => multi_root(folder, ignore, bases),
            None => Err(format!(
                "{} is not in a git repository and has none in it",
                shown(&dir)
            )),
        };
    };
    if let ([top], false) = (tops.as_slice(), shared(home)) {
        let name = folder_name(top);
        let base = base_of(&name, top, bases)?;
        return Ok(Root {
            path: top.clone(),
            multi: false,
            repos: vec![Found {
                name,
                main: top.clone(),
                base,
            }],
        });
    }
    let mut repos = if shared(home) {
        child_repos(home)
    } else {
        Vec::new()
    };
    for top in &tops {
        let name = folder_name(top);
        match repos.iter().find(|(n, _)| *n == name) {
            None => repos.push((name, top.clone())),
            // Repositories go by folder name in a feature's folder.
            Some((_, main)) if main != top => tracing::warn!(
                "leaving {} out of new features: {} has its name",
                top.display(),
                main.display()
            ),
            Some(_) => {}
        }
    }
    repos.sort();
    root_of(home, repos, ignore, bases)
}

/// `find_root` for a project among others, whose workspaces are `other_dirs`:
/// the repositories another project has as a workspace are that project's, so
/// they stay out.
///
/// Having one as a workspace too doesn't make it this project's when it sits
/// outside this root: such a workspace is a shortcut to the other project. It
/// is this project's when it sits in this root as well, since projects can
/// share a folder. The repository it was started from always stays, and a
/// root keeps all it found rather than be left with none.
pub fn find_project_root(
    dir: &Path,
    workspace_dirs: &[PathBuf],
    other_dirs: &[PathBuf],
    ignore: &[String],
    bases: &[(String, String)],
) -> Result<Root, String> {
    let mut root = find_root(dir, workspace_dirs, ignore, bases)?;
    let tops = |dirs: &[PathBuf]| -> Vec<PathBuf> {
        (dirs.iter())
            .filter_map(|d| main_top(&above_features(d)))
            .collect()
    };
    let (own, others) = (tops(workspace_dirs), tops(other_dirs));
    let here = main_top(&above_features(dir));
    let kept: Vec<Found> = (root.repos.iter())
        .filter(|r| {
            !others.contains(&r.main)
                || here.as_ref() == Some(&r.main)
                || (own.contains(&r.main) && r.main.starts_with(&root.path))
        })
        .cloned()
        .collect();
    if !kept.is_empty() {
        root.repos = kept;
    }
    Ok(root)
}

/// The folder holding `dir`'s nearest `.features` folder, else `dir`.
fn above_features(dir: &Path) -> PathBuf {
    dir.ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == FEATURES_DIR))
        .and_then(Path::parent)
        .unwrap_or(dir)
        .to_path_buf()
}

/// The root `dir` holding several repositories.
fn multi_root(dir: &Path, ignore: &[String], bases: &[(String, String)]) -> Result<Root, String> {
    let path = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let repos = child_repos(&path);
    root_of(&path, repos, ignore, bases)
}

/// The root at `path` of these repositories, less those in `ignore`.
fn root_of(
    path: &Path,
    found: Vec<(String, PathBuf)>,
    ignore: &[String],
    bases: &[(String, String)],
) -> Result<Root, String> {
    let mut repos = Vec::new();
    for (name, main) in found {
        if ignore.contains(&name) {
            continue;
        }
        let base = base_of(&name, &main, bases)?;
        repos.push(Found { name, main, base });
    }
    if repos.is_empty() {
        return Err(format!(
            "{} has no git repositories left once the ignored ones are out",
            shown(path)
        ));
    }
    Ok(Root {
        path: path.to_path_buf(),
        multi: true,
        repos,
    })
}

/// The repositories right under `dir`, by name: folders whose `.git` is a
/// folder (main worktrees; a `.git` file is a linked worktree's).
pub(super) fn child_repos(dir: &Path) -> Vec<(String, PathBuf)> {
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
pub fn main_top(dir: &Path) -> Option<PathBuf> {
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

/// The branches a feature in `main`'s repository can start from, the newest
/// commit first: local ones and origin's, each name once (`commit_of` takes
/// the local one first).
pub fn branches_of(main: &Path) -> Result<Vec<String>, String> {
    let refs = git(
        main,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    )?;
    let mut seen = std::collections::BTreeSet::new();
    Ok(refs
        .lines()
        .filter_map(|r| {
            r.strip_prefix("refs/heads/")
                .or_else(|| r.strip_prefix("refs/remotes/origin/"))
        })
        // origin's own pointer at its default branch.
        .filter(|name| *name != "HEAD" && seen.insert(name.to_string()))
        .map(str::to_owned)
        .collect())
}

/// Start `found` from the branch origin says is its default now, when it
/// has that branch here: a clone's `origin/HEAD` stays what it was when
/// cloned. Asks over the network, so it can take a while. Unreachable or not
/// fetched, what git has here stands, and the log says why.
pub fn follow_origin(found: &mut Found) {
    match crate::git_ops::remote_default(&found.main) {
        Ok(Some(branch)) if branches_of(&found.main).is_ok_and(|all| all.contains(&branch)) => {
            found.base = branch;
        }
        Ok(Some(branch)) => {
            tracing::warn!("{}: origin's default {branch} isn't fetched", found.name);
        }
        Ok(None) => {}
        Err(why) => tracing::warn!("asking origin for the default of {}: {why}", found.name),
    }
}

/// What `create` did to files outside the feature's folder, to tell the
/// user (checklist D1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Created {
    pub repos: Vec<Repo>,
    /// `<root>/.features/<slug>`.
    pub dir: PathBuf,
    /// The ignore files `.features/` was added to (`.git/info/exclude`,
    /// `.cursorindexingignore`, `.graphifyignore`, `pyrightconfig.json`):
    /// only those that changed.
    pub ignores: Vec<PathBuf>,
    /// The root's files and folders other than repositories (AGENTS.md,
    /// .agents) linked into the feature folder, multi-repository only.
    pub links: Vec<PathBuf>,
}

/// Make feature `slug`'s folder under `root`: a worktree on branch
/// `feature/<slug>` from its base for each repository named in `touched`,
/// and none for the rest (`add_repos` brings one in later). A single
/// repository is always in, and its worktree is the folder itself. Keeps
/// `.features/` out of git and the tools that index the root. On a failure,
/// what was made is taken down again before the error returns.
pub fn create(root: &Root, slug: &str, touched: &[String]) -> Result<Created, String> {
    create_in(root, slug, touched, None)
}

/// `create` for linked project `project` of a feature: its repositories
/// are `project`'s (`Repo::project`), and only those named in `touched`
/// are in it, a single one too (the home project's single repository
/// always is: it is what the feature is about). At least one must be.
pub fn create_linked(
    root: &Root,
    slug: &str,
    touched: &[String],
    project: &str,
) -> Result<Created, String> {
    create_in(root, slug, touched, Some(project))
}

fn create_in(
    root: &Root,
    slug: &str,
    touched: &[String],
    project: Option<&str>,
) -> Result<Created, String> {
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
    let chosen = chosen(root, touched, project);
    if chosen.is_empty() && project.is_some() {
        return Err(format!("no repository of {} was picked", shown(&root.path)));
    }
    let features = root.path.join(FEATURES_DIR);
    let dir = features.join(slug);
    if dir.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", shown(&dir)));
    }
    let branch = format!("feature/{slug}");
    for repo in &chosen {
        if branch_exists(&repo.main, &branch) {
            return Err(format!("{} already has a branch {branch}", repo.name));
        }
    }
    let mut made = Made {
        features: (!features.exists()).then(|| features.clone()),
        dir: Some(dir.clone()),
        branch: branch.clone(),
        worktrees: Vec::new(),
        kept_branch: false,
    };
    match build(root, &dir, &branch, &chosen, project, &mut made) {
        Ok(created) => Ok(created),
        Err(err) => {
            made.undo();
            Err(err)
        }
    }
}

/// The repositories of `root` a new feature folder holds: those named in
/// `touched`, and the home project's single one whatever it says.
fn chosen<'a>(root: &'a Root, touched: &[String], project: Option<&str>) -> Vec<&'a Found> {
    let all = !root.multi && project.is_none();
    (root.repos.iter())
        .filter(|r| all || touched.contains(&r.name))
        .collect()
}

/// A linked project to make a feature's folder in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpec {
    /// The project's id.
    pub project: String,
    /// Its name, for errors.
    pub name: String,
    pub root: Root,
    /// Its repositories the feature holds, on the feature's branch.
    pub touched: Vec<String>,
}

/// Repositories to bring into a feature folder that is there already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddSpec {
    /// The linked project's id; `None`: the home project.
    pub project: Option<String>,
    /// The project's name, for errors.
    pub name: String,
    pub root: Root,
    pub names: Vec<String>,
}

/// What `link_more` made: new links with their folders, and repositories
/// that joined folders there already.
pub type MoreLinked = (Vec<(Link, Created)>, Vec<Repo>);

/// `create_links` for `links`, then `add_repos` for `adds`: on a failure,
/// everything made so far is taken down again.
pub fn link_more(
    feature: &Feature,
    links: &[LinkSpec],
    adds: &[AddSpec],
) -> Result<MoreLinked, String> {
    let made = create_links(&feature.slug, links)?;
    let mut repos: Vec<Repo> = Vec::new();
    for add in adds {
        match add_repos(feature, add.project.as_deref(), &add.root, &add.names) {
            Ok(added) => repos.extend(added),
            Err(err) => {
                let mut failed = Vec::new();
                for repo in &repos {
                    take_down_repo(&feature.branch(), repo, true, &mut failed, feature.pull_of(&repo.key()));
                }
                if !failed.is_empty() {
                    tracing::warn!("taking repositories out again: {}", failed.join("; "));
                }
                for (_, created) in &made {
                    undo_created(&feature.slug, created);
                }
                return Err(format!("{}: {err}", add.name));
            }
        }
    }
    Ok((made, repos))
}

/// `create` in the home `root`, then `create_links`: on a failure, every
/// folder made so far is taken down again.
pub fn create_spanning(
    root: &Root,
    slug: &str,
    touched: &[String],
    links: &[LinkSpec],
) -> Result<(Created, Vec<(Link, Created)>), String> {
    let home = create(root, slug, touched)?;
    match create_links(slug, links) {
        Ok(made) => Ok((home, made)),
        Err(err) => {
            undo_created(slug, &home);
            Err(err)
        }
    }
}

/// `create_linked` for each of `links`, the error naming the project: on
/// a failure, the folders made before it are taken down again.
pub fn create_links(slug: &str, links: &[LinkSpec]) -> Result<Vec<(Link, Created)>, String> {
    let mut made: Vec<(Link, Created)> = Vec::new();
    for spec in links {
        match create_linked(&spec.root, slug, &spec.touched, &spec.project) {
            Ok(created) => made.push((link_of(&spec.project, &spec.root, &created), created)),
            Err(err) => {
                for (_, created) in &made {
                    undo_created(slug, created);
                }
                return Err(format!("{}: {err}", spec.name));
            }
        }
    }
    Ok(made)
}

/// Take down a feature folder `create` made a moment ago.
fn undo_created(slug: &str, created: &Created) {
    let repos: Vec<&Repo> = created.repos.iter().collect();
    let mut failed = Vec::new();
    take_down(
        &format!("feature/{slug}"),
        &repos,
        &created.dir,
        true,
        &mut failed,
        &[],
    );
    if !failed.is_empty() {
        tracing::warn!(
            "taking {} down again: {}",
            shown(&created.dir),
            failed.join("; ")
        );
    }
}

/// The link of linked project `project` whose feature folder
/// `create_linked` made under `root` as `created`.
pub fn link_of(project: &str, root: &Root, created: &Created) -> Link {
    Link {
        project: project.to_owned(),
        root: root.path.clone(),
        dir: created.dir.clone(),
        multi: root.multi,
    }
}

/// What `create` or `add_repos` has made so far, to take down on a failure.
struct Made {
    /// `.features`, when `create` made it.
    features: Option<PathBuf>,
    /// The feature folder, when `create` made it.
    dir: Option<PathBuf>,
    /// The branch each worktree was made on.
    branch: String,
    /// Each worktree's main worktree and folder.
    worktrees: Vec<(PathBuf, PathBuf)>,
    /// The branch was there before: taking down leaves it.
    kept_branch: bool,
}

impl Made {
    fn undo(self) {
        for (main, path) in self.worktrees.iter().rev() {
            let p = path.to_string_lossy();
            if let Err(err) = git(main, &["worktree", "remove", "--force", &p]) {
                tracing::warn!("taking down {}: {err}", path.display());
            }
            if !self.kept_branch
                && let Err(err) = git(main, &["branch", "-D", &self.branch])
            {
                tracing::warn!("deleting {} in {}: {err}", self.branch, main.display());
            }
        }
        if let Some(dir) = &self.dir
            && let Err(err) = std::fs::remove_dir_all(dir)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("removing {}: {err}", dir.display());
        }
        for (main, _) in &self.worktrees {
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
    chosen: &[&Found],
    project: Option<&str>,
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
    for found in chosen {
        let path = if root.multi {
            dir.join(&found.name)
        } else {
            dir.to_path_buf()
        };
        repos.push(add_worktree(found, path, project, made)?);
    }
    let links = if root.multi {
        link_root(&root.path, dir)?
    } else {
        Vec::new()
    };
    let ignores = keep_out(root)?;
    for repo in &repos {
        guard_worktree(repo, branch);
    }
    Ok(Created {
        repos,
        dir: dir.to_path_buf(),
        ignores,
        links,
    })
}

/// A worktree at `path` of repository `found` on `made`'s branch, from its
/// base; noted in `made`.
fn add_worktree(
    found: &Found,
    path: PathBuf,
    project: Option<&str>,
    made: &mut Made,
) -> Result<Repo, String> {
    let commit =
        commit_of(&found.main, &found.base).map_err(|err| format!("{}: {err}", found.name))?;
    let branch = made.branch.clone();
    let p = path.to_string_lossy();
    git(
        &found.main,
        &["worktree", "add", "-q", "-b", &branch, &p, &commit],
    )
    .map_err(|err| format!("{}: {err}", found.name))?;
    made.worktrees.push((found.main.clone(), path.clone()));
    Ok(Repo {
        name: found.name.clone(),
        main: found.main.clone(),
        base: found.base.clone(),
        base_commit: commit,
        worktree: path,
        touched: true,
        branch: Some(branch),
        project: project.map(str::to_owned),
    })
}

/// The folder and worktree of a review of pull request `pull` of
/// repository `repo` of `root`: `<root>/.features/<slug>`, as a feature's
/// is made, with one repository only. Its head is fetched first, into
/// `PullRef::fetch_ref` (a fork's too), and its base into origin's branch.
///
/// Someone else's pull request gets branch `gerdr/pr-<n>` at its head,
/// guarded so it is never pushed. One of the user's own from this
/// repository gets its own branch: the one already here if there is one,
/// else a new one from origin's, and no guard, as the user pushes it.
/// Its `Repo::base` is origin's base branch, as GitHub diffs against it.
#[cfg(test)]
pub fn create_pr(root: &Root, repo: &str, pull: &PullRef, slug: &str) -> Result<Created, String> {
    create_pr_in(root, &[(repo.to_owned(), pull.clone())], slug, None)
}

/// `create_pr` for several pull requests, each of the repository of
/// `root` it names, in linked project `project` if one (`Repo::project`).
/// A root of several may have none: its folder is made all the same.
fn create_pr_in(
    root: &Root,
    picks: &[(String, PullRef)],
    slug: &str,
    project: Option<&str>,
) -> Result<Created, String> {
    if picks.is_empty() && !root.multi {
        return Err(format!("{} has no pull request picked", shown(&root.path)));
    }
    let features = root.path.join(FEATURES_DIR);
    let dir = features.join(slug);
    if dir.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", shown(&dir)));
    }
    let chosen = pr_chosen(root, picks)?;
    // Branches differ per repository: `Made` deletes none, `pr_worktrees`
    // notes those to.
    let mut made = Made {
        features: (!features.exists()).then(|| features.clone()),
        dir: Some(dir.clone()),
        branch: String::new(),
        worktrees: Vec::new(),
        kept_branch: true,
    };
    let mut new_branches: Vec<(PathBuf, String)> = Vec::new();
    let built = (|| {
        if root.multi {
            std::fs::create_dir_all(&dir)
                .map_err(|err| format!("couldn't make {}: {err}", shown(&dir)))?;
        } else if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("couldn't make {}: {err}", shown(parent)))?;
        }
        let repos = pr_worktrees(root, &dir, &chosen, project, &mut made, &mut new_branches)?;
        let links = match root.multi {
            true => link_root(&root.path, &dir)?,
            false => Vec::new(),
        };
        let ignores = keep_out(root)?;
        Ok(Created {
            repos,
            dir: dir.clone(),
            ignores,
            links,
        })
    })();
    if built.is_err() {
        made.undo();
        delete_branches(&new_branches);
    }
    built
}

/// A pull request to make a worktree of: the repository found, its
/// branch, and whether that branch is there already.
type PrChosen<'a> = (&'a Found, &'a PullRef, String, bool);

/// The repositories of `root` that `picks` (repository name, pull request)
/// name, once each pull request is fetched; or why one can't be had.
fn pr_chosen<'a>(root: &'a Root, picks: &'a [(String, PullRef)]) -> Result<Vec<PrChosen<'a>>, String> {
    let mut chosen = Vec::new();
    for (repo, pull) in picks {
        let Some(found) = root.repos.iter().find(|r| r.name == *repo) else {
            return Err(format!("{} has no repository {repo}", shown(&root.path)));
        };
        let branch = pull.branch();
        let kept = branch_exists(&found.main, &branch);
        if kept && !pull.own() {
            return Err(format!("{repo} already has a branch {branch}"));
        }
        chosen.push((found, pull, branch, kept));
    }
    for (found, pull, ..) in &chosen {
        fetch_pull(&found.main, pull).map_err(|err| format!("{}: {err}", found.name))?;
    }
    Ok(chosen)
}

/// The worktrees of `chosen` in feature folder `dir` of `root`, each on its
/// pull request's branch (see `create_pr`), in linked project `project` if
/// one. What it makes goes into `made` and `new_branches`.
fn pr_worktrees(
    root: &Root,
    dir: &Path,
    chosen: &[PrChosen],
    project: Option<&str>,
    made: &mut Made,
    new_branches: &mut Vec<(PathBuf, String)>,
) -> Result<Vec<Repo>, String> {
    let mut repos = Vec::new();
    for (found, pull, branch, kept) in chosen {
        let name = &found.name;
        let base = format!("origin/{}", pull.base_ref);
        let base_commit = commit_of(&found.main, &base).map_err(|err| format!("{name}: {err}"))?;
        let path = if root.multi {
            dir.join(name)
        } else {
            dir.to_owned()
        };
        let p = path.to_string_lossy();
        let from = match pull.own() {
            true => format!("origin/{}", pull.head_ref),
            false => pull.head_sha.clone(),
        };
        let args: Vec<&str> = match kept {
            true => vec!["worktree", "add", "-q", &p, branch],
            false => vec!["worktree", "add", "-q", "-b", branch, &p, &from],
        };
        git(&found.main, &args).map_err(|err| format!("{name}: {err}"))?;
        made.worktrees.push((found.main.clone(), path.clone()));
        if !kept {
            new_branches.push((found.main.clone(), branch.clone()));
        }
        // The user's branch here may be behind what they pushed; one gone
        // its own way stays as it is, its commits theirs.
        if *kept && let Err(err) = git(&path, &["merge", "-q", "--ff-only", &from]) {
            tracing::warn!("{branch} of {name} stays where it was: {err}");
        }
        let repo = Repo {
            name: name.clone(),
            main: found.main.clone(),
            base,
            base_commit,
            worktree: path,
            touched: true,
            branch: Some(branch.clone()),
            project: project.map(str::to_owned),
        };
        if !pull.own() {
            guard_worktree(&repo, branch);
        }
        repos.push(repo);
    }
    Ok(repos)
}

/// Delete `branches` (main worktree, name) made a moment ago.
fn delete_branches(branches: &[(PathBuf, String)]) {
    for (main, branch) in branches {
        if let Err(err) = hook::unguard(main, branch) {
            tracing::warn!("unguarding {branch} in {}: {err}", main.display());
        }
        if let Err(err) = git(main, &["branch", "-D", branch]) {
            tracing::warn!("deleting {branch} in {}: {err}", main.display());
        }
    }
}

/// Worktrees of the pull requests `picks` of `root`'s repositories in a
/// review's folder `dir` that is there already; on a failure, none.
fn add_pr_repos(
    root: &Root,
    dir: &Path,
    picks: &[(String, PullRef)],
    project: Option<&str>,
) -> Result<Vec<Repo>, String> {
    if !root.multi {
        return Err(format!("{} is a single repository's folder", shown(dir)));
    }
    if !dir.is_dir() {
        return Err(format!("{} isn't there", shown(dir)));
    }
    if let Some((name, _)) = picks.iter().find(|(name, _)| dir.join(name).exists()) {
        return Err(format!("{} already has {name}", shown(dir)));
    }
    let chosen = pr_chosen(root, picks)?;
    let mut made = Made {
        features: None,
        dir: None,
        branch: String::new(),
        worktrees: Vec::new(),
        kept_branch: true,
    };
    let mut new_branches = Vec::new();
    let added = pr_worktrees(root, dir, &chosen, project, &mut made, &mut new_branches);
    if added.is_err() {
        made.undo();
        delete_branches(&new_branches);
    }
    added
}

/// Pull requests to add to an open review, of one project's root.
#[derive(Clone, Debug)]
pub struct PrAdd {
    /// The linked project's id; `None`: the home one.
    pub project: Option<String>,
    /// The project's name, for errors.
    pub name: String,
    pub root: Root,
    /// By repository name, each `PullRef::repo` its key.
    pub picks: Vec<(String, PullRef)>,
}

/// Add the pull requests `adds` to review `feature`: into its folders, or a
/// new one for a project it doesn't span yet. On a failure, all that was
/// added is taken down again.
pub fn add_review(feature: &Feature, adds: &[PrAdd]) -> Result<MoreLinked, String> {
    let mut made: Vec<(Link, Created)> = Vec::new();
    let mut repos: Vec<Repo> = Vec::new();
    for add in adds {
        let link = (add.project.as_ref()).map(|p| feature.links.iter().find(|l| l.project == *p));
        let result = match (&add.project, link.flatten()) {
            (None, _) => add_pr_repos(&add.root, &feature.dir, &add.picks, None).map(Err),
            (Some(p), Some(link)) => add_pr_repos(&add.root, &link.dir, &add.picks, Some(p)).map(Err),
            (Some(p), None) => create_pr_in(&add.root, &add.picks, &feature.slug, Some(p))
                .map(|created| Ok((link_of(p, &add.root, &created), created))),
        };
        match result {
            Ok(Ok(linked)) => made.push(linked),
            Ok(Err(added)) => repos.extend(added),
            Err(err) => {
                let pulls: Vec<PullRef> = (adds.iter())
                    .flat_map(|a| a.picks.iter().map(|(_, p)| p.clone()))
                    .collect();
                let mut failed = Vec::new();
                for repo in &repos {
                    let pull = pulls.iter().find(|p| p.repo == repo.key());
                    take_down_repo("", repo, true, &mut failed, pull);
                }
                if !failed.is_empty() {
                    tracing::warn!("taking repositories out again: {}", failed.join("; "));
                }
                for (_, created) in &made {
                    undo_review(created, &pulls);
                }
                return Err(format!("{}: {err}", add.name));
            }
        }
    }
    Ok((made, repos))
}

/// A linked project's part of a review: its root, the pull requests picked
/// in it.
#[derive(Clone, Debug)]
pub struct LinkedPicks {
    pub spec: LinkSpec,
    /// By repository name.
    pub picks: Vec<(String, PullRef)>,
}

/// The folders of review `slug` of the pull requests `picks` of the home
/// `root`'s repositories, by name, and of `links`' (see `create_pr`). On a
/// failure, every folder made so far is taken down again.
pub fn create_review(
    root: &Root,
    picks: &[(String, PullRef)],
    links: &[LinkedPicks],
    slug: &str,
) -> Result<(Created, Vec<(Link, Created)>), String> {
    let home = create_pr_in(root, picks, slug, None)?;
    let mut made: Vec<(Link, Created)> = Vec::new();
    for linked in links {
        let spec = &linked.spec;
        match create_pr_in(&spec.root, &linked.picks, slug, Some(&spec.project)) {
            Ok(created) => made.push((link_of(&spec.project, &spec.root, &created), created)),
            Err(err) => {
                let pulls: Vec<PullRef> = (picks.iter())
                    .chain(links.iter().flat_map(|l| l.picks.iter()))
                    .map(|(_, p)| p.clone())
                    .collect();
                for created in std::iter::once(&home).chain(made.iter().map(|(_, c)| c)) {
                    undo_review(created, &pulls);
                }
                return Err(format!("{}: {err}", spec.name));
            }
        }
    }
    Ok((home, made))
}

/// Take down a review's folder `create_pr_in` made a moment ago, keeping
/// the user's own branches: `pulls` are its pull requests, `PullRef::repo`
/// their repositories' keys.
fn undo_review(created: &Created, pulls: &[PullRef]) {
    let repos: Vec<&Repo> = created.repos.iter().collect();
    let mut failed = Vec::new();
    take_down("", &repos, &created.dir, true, &mut failed, pulls);
    if !failed.is_empty() {
        tracing::warn!(
            "taking {} down again: {}",
            shown(&created.dir),
            failed.join("; ")
        );
    }
}

/// Where a review's worktree went once moved to its pull request's head on
/// GitHub (`update_pr`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedPr {
    pub head_sha: String,
    /// `origin/<base>`, as `Repo::base` has it.
    pub base: String,
    pub base_ref: String,
    pub base_commit: String,
}

/// Move `repo`, the worktree of pull request `pull`'s review, to the head
/// and base `to` says GitHub has now. It is fetched first. Nothing of the
/// user's is lost: uncommitted changes refuse it, as do commits of theirs
/// on someone else's pull request (its branch then moves even when the
/// author rewrote theirs); the user's own goes on only by fast-forward.
pub fn update_pr(repo: &Repo, pull: &PullRef, to: &super::pr::Remote) -> Result<MovedPr, String> {
    let dir = &repo.worktree;
    let name = repo.key();
    if !dir.is_dir() {
        return Err(format!("{name}: {} isn't there", shown(dir)));
    }
    let status = git(dir, &["status", "--porcelain", "--untracked-files=no"])?;
    if !status.trim().is_empty() {
        return Err(format!("{name} has uncommitted changes: commit or drop them first"));
    }
    if !pull.own() {
        let range = format!("{}..HEAD", pull.head_sha);
        let mine = git(dir, &["rev-list", "--count", &range])?;
        let mine: usize = mine.trim().parse().unwrap_or(0);
        if mine > 0 {
            return Err(format!(
                "{name} has {mine} {} of yours not in #{}",
                plural(mine, "commit"),
                pull.number
            ));
        }
    }
    let mut next = pull.clone();
    next.moved_to(&to.head_sha, &to.base_ref);
    fetch_pull(&repo.main, &next).map_err(|err| format!("{name}: {err}"))?;
    match pull.own() {
        true => {
            let theirs = format!("origin/{}", next.head_ref);
            git(dir, &["merge", "-q", "--ff-only", &theirs]).map_err(|_| {
                format!(
                    "{} went its own way from GitHub's: merge or rebase {theirs} in its worktree",
                    next.head_ref
                )
            })?;
        }
        false => {
            git(dir, &["reset", "-q", "--keep", &next.head_sha])
                .map_err(|err| format!("{name}: {err}"))?;
        }
    }
    let head = git(dir, &["rev-parse", "HEAD"])?.trim().to_owned();
    let base = format!("origin/{}", next.base_ref);
    let base_commit = commit_of(&repo.main, &base).map_err(|err| format!("{name}: {err}"))?;
    Ok(MovedPr {
        head_sha: head,
        base,
        base_ref: next.base_ref,
        base_commit,
    })
}

/// Fetch pull request `pull`'s head into its `fetch_ref`, and its base (and
/// for one of the user's own, its branch) into origin's branches; then make
/// sure its head commit came.
/// How long fetching a pull request may take.
const FETCH_TIME: Duration = Duration::from_secs(120);

fn fetch_pull(main: &Path, pull: &PullRef) -> Result<(), String> {
    let head = format!("+refs/pull/{}/head:{}", pull.number, pull.fetch_ref());
    let base = format!(
        "+refs/heads/{0}:refs/remotes/origin/{0}",
        pull.base_ref
    );
    let own = format!(
        "+refs/heads/{0}:refs/remotes/origin/{0}",
        pull.head_ref
    );
    let mut args = vec!["fetch", "-q", "origin", &head, &base];
    if pull.own() {
        args.push(&own);
    }
    git_within(main, &args, FETCH_TIME)
        .map_err(|err| format!("couldn't fetch #{}: {err}", pull.number))?;
    let spec = format!("{}^{{commit}}", pull.head_sha);
    git(main, &["rev-parse", "--verify", "--quiet", &spec]).map_err(|_| {
        format!(
            "#{} moved on while it was fetched: try again",
            pull.number
        )
    })?;
    Ok(())
}

/// Bring repositories `names` of `root` into `feature`'s existing folder of
/// `project` (`None`: the home one), each on the feature's branch from its
/// base. Returns them, for the caller to keep (`Feature::insert_repos`). On
/// a failure, those made are taken down again before the error returns.
pub fn add_repos(
    feature: &Feature,
    project: Option<&str>,
    root: &Root,
    names: &[String],
) -> Result<Vec<Repo>, String> {
    let (dir, multi, at) = match project {
        None => (&feature.dir, feature.multi, &feature.root),
        Some(id) => {
            let Some(link) = feature.link(id) else {
                return Err(format!("{} doesn't span {id}", feature.title));
            };
            (&link.dir, link.multi, &link.root)
        }
    };
    if !multi {
        return Err(format!("{} holds one repository only", shown(dir)));
    }
    if root.path != *at {
        return Err(format!(
            "its repositories moved from {} to {}",
            shown(at),
            shown(&root.path)
        ));
    }
    let branch = feature.branch();
    let mut chosen = Vec::new();
    for name in names {
        let Some(found) = root.repos.iter().find(|r| r.name == *name) else {
            return Err(format!("{} has no repository {name}", shown(&root.path)));
        };
        if feature.repos_of(project).any(|r| r.name == *name) {
            return Err(format!("{} already has {name}", feature.title));
        }
        // A root file of that name, linked in when the folder was made.
        let path = dir.join(name);
        if path.symlink_metadata().is_ok() {
            return Err(format!("{} already exists", shown(&path)));
        }
        if branch_exists(&found.main, &branch) {
            return Err(format!("{name} already has a branch {branch}"));
        }
        chosen.push((found, path));
    }
    let mut made = Made {
        features: None,
        dir: None,
        branch: branch.clone(),
        worktrees: Vec::new(),
        kept_branch: false,
    };
    let mut repos = Vec::new();
    for (found, path) in chosen {
        match add_worktree(found, path, project, &mut made) {
            Ok(repo) => repos.push(repo),
            Err(err) => {
                made.undo();
                return Err(err);
            }
        }
    }
    for repo in &repos {
        guard_worktree(repo, &branch);
    }
    Ok(repos)
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

/// What `repo`'s change is counted from (its commits, diffs and patch-id):
/// where its branch last met its base, the local one or origin's, whichever
/// it shares more with; else its base branch if it is there, else origin's,
/// else the commit it started at.
pub(super) fn base_ref(repo: &Repo) -> String {
    let (first, names) = base_names(repo);
    if names.is_empty() {
        return first;
    }
    // A branch that took origin's base in while the local base stayed behind
    // counts from what it took in: origin's commits aren't its change, and
    // pulling the local base after changes nothing.
    if let Some(branch) = repo.branch.as_deref().filter(|_| repo.touched) {
        // A base its branch merged into (its pull request, fast-forwarded or
        // by a merge commit) would leave it no change: what it shipped counts
        // from where it met the others, else from where it started.
        let start = repo.base_commit.as_str();
        let range = format!("{start}..{branch}");
        let count = || git(&repo.main, &["rev-list", "--count", &range]);
        let shipped = !start.is_empty() && count().is_ok_and(|n| n.trim() != "0");
        let met: Vec<&str> = (names.iter().map(String::as_str))
            .filter(|name| !(shipped && has_branch(repo, name, branch)))
            .collect();
        let mut args = vec!["merge-base", branch];
        match met.is_empty() {
            true => args.push(start),
            false => args.extend(met),
        }
        if let Ok(fork) = git(&repo.main, &args) {
            return fork.trim().to_owned();
        }
    }
    first
}

/// Whether commit `base` of `repo` has all of `branch`: it is in its history.
fn has_branch(repo: &Repo, base: &str, branch: &str) -> bool {
    git(&repo.main, &["merge-base", "--is-ancestor", branch, base]).is_ok()
}

/// `repo`'s base itself, not where its branch met it: the ref shown from
/// another machine, else its base branch if it is there, else origin's, else
/// the commit it started at.
pub(super) fn base_tip(repo: &Repo) -> String {
    base_names(repo).0
}

/// `base_tip`, with the base branch names that are there (none when the tip
/// is the ref from another machine or the start commit).
fn base_names(repo: &Repo) -> (String, Vec<String>) {
    // Shown here, written on another machine: where it left off there, whatever
    // the same branch here is at.
    if let Some(seen) = repo.branch.as_deref().map(super::remote::view_base_ref) {
        let spec = format!("{seen}^{{commit}}");
        if git(&repo.main, &["rev-parse", "--verify", "--quiet", &spec]).is_ok() {
            return (seen, Vec::new());
        }
    }
    let names: Vec<String> = [repo.base.clone(), format!("origin/{}", repo.base)]
        .into_iter()
        .filter(|name| {
            let spec = format!("{name}^{{commit}}");
            git(&repo.main, &["rev-parse", "--verify", "--quiet", &spec]).is_ok()
        })
        .collect();
    match names.first() {
        Some(first) => (first.clone(), names),
        None => (repo.base_commit.clone(), Vec::new()),
    }
}

/// Root entries a multi-repository feature folder links to: not
/// repositories or their worktrees, and not these.
const UNLINKED: [&str; 4] = [FEATURES_DIR, ".git", "graphify-out", "node_modules"];

/// The link at the top of a feature folder to the feature's data folder and
/// its stage files (see `link_docs`).
pub const DOCS_LINK: &str = "feature-docs";

/// Link the root's files and folders other than repositories into `dir`.
fn link_root(root: &Path, dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(root).map_err(|err| format!("couldn't list {}: {err}", shown(root)))?;
    let mut names: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .filter(|name| {
            let path = root.join(name);
            // A repository from elsewhere may have taken the name.
            !UNLINKED.iter().any(|u| name == *u)
                && path.join(".git").symlink_metadata().is_err()
                && dir.join(name).symlink_metadata().is_err()
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

/// The repositories in `feature`'s root it neither holds nor has as context,
/// by name, with their main worktrees. No git, only the folder's listing.
pub fn others_in_root(feature: &Feature) -> Vec<(String, PathBuf)> {
    if !feature.multi {
        return Vec::new();
    }
    child_repos(&feature.root)
        .into_iter()
        .filter(|(name, _)| !feature.repos_of(None).any(|r| r.name == *name))
        .filter(|(name, _)| !feature.context.iter().any(|c| c.key() == *name))
        .collect()
}

/// Keep `.features/` out of git (single repository) and the tools that
/// index the root; the files that changed.
fn keep_out(root: &Root) -> Result<Vec<PathBuf>, String> {
    let mut changed = Vec::new();
    if !root.multi {
        changed.extend(exclude_line(&root.path, "/.features/")?);
    }
    match cursor_index_rule(&root.path) {
        Ok(file) => changed.extend(file),
        Err(err) => tracing::warn!("{err}"),
    }
    // The tools' files are only edited when there; one that can't be is
    // logged and left, as a feature can do without it.
    for name in [".graphifyignore", "pyrightconfig.json"] {
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

/// Keep `.features/` out of what Cursor indexes in `root`, in a project that
/// uses Cursor. Not `.cursorignore`: that one also denies its agents every
/// read and edit under the folder, from inside it too, and they go round by
/// the shell. The file, when it changed.
fn cursor_index_rule(root: &Path) -> Result<Option<PathBuf>, String> {
    let file = root.join(CURSOR_INDEX);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("couldn't read {}: {err}", shown(&file)));
        }
        Err(_) if root.join(CURSOR_DENY).exists() => String::new(),
        Err(_) => return Ok(None),
    };
    let Some(text) = add_line(&text, ".features/") else {
        return Ok(None);
    };
    std::fs::write(&file, text).map_err(|err| format!("couldn't write {}: {err}", shown(&file)))?;
    Ok(Some(file))
}

/// Take `.features` out of `root`'s `.cursorignore`, where gerdr once put it,
/// and into what Cursor doesn't index (see `cursor_index_rule`). Whether it
/// was there. Touches files: off the UI thread.
pub fn free_for_cursor(root: &Path) -> Result<bool, String> {
    let file = root.join(CURSOR_DENY);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("couldn't read {}: {err}", shown(&file))),
    };
    let Some(text) = without_features(&text) else {
        return Ok(false);
    };
    // In this order: Cursor never gets to index the folder between the two.
    cursor_index_rule(root)?;
    std::fs::write(&file, text).map_err(|err| format!("couldn't write {}: {err}", shown(&file)))?;
    Ok(true)
}

/// `text` without its lines that ignore all of `.features`, every other byte
/// kept, unless it has none.
fn without_features(text: &str) -> Option<String> {
    let ignores = |line: &str| {
        let pattern = unanchored(line).trim_end_matches("/**");
        pattern.trim_end_matches('/') == FEATURES_DIR
    };
    let kept: String = text.split_inclusive('\n').filter(|l| !ignores(l)).collect();
    (kept.len() != text.len()).then_some(kept)
}

/// `line` in the git exclude file of the repository at `repo`, shared by
/// its worktrees: nothing tracked changes, so nothing of it can be
/// committed. The file, when it changed.
pub(super) fn exclude_line(repo: &Path, line: &str) -> Result<Option<PathBuf>, String> {
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let exclude = PathBuf::from(common.trim()).join("info").join("exclude");
    let text = match std::fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("couldn't read {}: {err}", shown(&exclude))),
    };
    let Some(text) = add_line(&text, line) else {
        return Ok(None);
    };
    if let Some(info) = exclude.parent() {
        std::fs::create_dir_all(info)
            .map_err(|err| format!("couldn't make {}: {err}", shown(info)))?;
    }
    std::fs::write(&exclude, text)
        .map_err(|err| format!("couldn't write {}: {err}", shown(&exclude)))?;
    Ok(Some(exclude))
}

/// Link feature folder `dir` to the feature's data folder `data` as
/// `DOCS_LINK`, so any agent started in it can name the stage files. It is
/// kept out of git for a single repository, where the folder is a worktree;
/// a multi-repository folder is no repository. The exclude file, when it
/// changed. Runs git: off the UI thread.
pub fn link_docs(dir: &Path, multi: bool, data: &Path) -> Result<Option<PathBuf>, String> {
    let link = dir.join(DOCS_LINK);
    let there = link.symlink_metadata().is_ok();
    if there && std::fs::read_link(&link).ok().as_deref() != Some(data) {
        return Err(format!("{} is already something else", shown(&link)));
    }
    // Its workspace outlived the folder: nothing to link.
    if !dir.is_dir() {
        return Ok(None);
    }
    // In this order: a link git could see is never made.
    let exclude = if multi {
        None
    } else {
        exclude_line(dir, &format!("/{DOCS_LINK}"))?
    };
    if !there {
        // A link to nothing makes the tools that list files complain.
        std::fs::create_dir_all(data)
            .map_err(|err| format!("couldn't make {}: {err}", shown(data)))?;
        std::os::unix::fs::symlink(data, &link)
            .map_err(|err| format!("couldn't link {}: {err}", shown(&link)))?;
    }
    Ok(exclude)
}

/// What a feature taken over from another machine needs of its folder, whose
/// worktrees came on their own: the guard on its branches, the root's links in a
/// folder of several repositories, the link to its files. A help to its agents, so
/// what fails is said in the log and the rest still goes.
pub fn settle_taken(feature: &Feature, data: &Path) {
    for repo in feature.repos.iter().filter(|r| r.touched) {
        if let Some(branch) = &repo.branch {
            guard_worktree(repo, branch);
        }
    }
    if feature.multi
        && let Err(why) = link_root(&feature.root, &feature.dir)
    {
        tracing::warn!("{}: {why}", feature.slug);
    }
    if let Err(why) = link_docs(&feature.dir, feature.multi, data) {
        tracing::warn!("{}: {why}", feature.slug);
    }
}

/// `pattern`, an ignore file's line, without what anchors it.
fn unanchored(pattern: &str) -> &str {
    let pattern = pattern.trim();
    let pattern = pattern.strip_prefix("**/").unwrap_or(pattern);
    pattern.strip_prefix('/').unwrap_or(pattern)
}

/// `text` with ignore pattern `line` added, unless a line already covers
/// it. A pattern ending in `/` covers folders only, and git takes a symlink
/// for a file: only a `line` for a folder is covered by one.
fn add_line(text: &str, line: &str) -> Option<String> {
    let name = unanchored(line).trim_end_matches('/');
    let folder = line.ends_with('/');
    let has = text
        .lines()
        .map(unanchored)
        .any(|l| l == name || (folder && l.strip_suffix('/') == Some(name)));
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

/// Put the worktree of `feature`'s repository `repo` (its `Repo::key`) on
/// branch `feature/<slug>` at the commit it is on, so the feature can
/// change it. Returns the repository as it is now.
pub fn touch(feature: &Feature, repo: &str) -> Result<Repo, String> {
    let Some(found) = feature.repo(repo) else {
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
        tracing::warn!("guarding {branch} of {}: {err}", repo.key());
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
/// commits not in dev"), named by `Repo::key`, over every project it
/// spans. Empty when nothing is lost.
pub fn losses(feature: &Feature) -> Vec<String> {
    (feature.repos.iter())
        .filter_map(|r| loss(r, feature.pull_of(&r.key())))
        .collect()
}

/// What taking `repo`'s worktree down would lose, as `losses` says it; of
/// pull request `pull`'s review, commits on GitHub aren't lost.
fn loss(repo: &Repo, pull: Option<&PullRef>) -> Option<String> {
    if !repo.worktree.is_dir() {
        return None;
    }
    let status = match git(
        &repo.worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    ) {
        Ok(status) => status,
        Err(err) => return Some(format!("{}: couldn't tell what it has ({err})", repo.key())),
    };
    let mut parts = Vec::new();
    let changed = status.lines().filter(|l| !l.trim().is_empty()).count();
    if changed > 0 {
        parts.push(format!("{changed} changed {}", plural(changed, "file")));
    }
    if repo.touched {
        match commits_of(repo, pull) {
            Ok(0) => {}
            Ok(n) => parts.push(format!(
                "{n} {} not in {}",
                plural(n, "commit"),
                kept_in(repo, pull)
            )),
            Err(err) => parts.push(format!("couldn't count its commits ({err})")),
        }
    }
    (!parts.is_empty()).then(|| format!("{}: {}", repo.key(), parts.join(", ")))
}

/// What keeps `repo`'s commits once its worktree goes, in words: its base,
/// or for pull request `pull`'s review, the pull request.
fn kept_in(repo: &Repo, pull: Option<&PullRef>) -> String {
    match pull {
        Some(pull) => format!("#{}", pull.number),
        None => repo.base.clone(),
    }
}

/// How many commits `repo`'s worktree has that its base doesn't: none once
/// its branch is merged, a squash being one whose commits stay outside it.
/// Of pull request `pull`'s review: those its head on GitHub doesn't have.
fn commits_of(repo: &Repo, pull: Option<&PullRef>) -> Result<usize, String> {
    if let Some(pull) = pull {
        let range = format!("{}..HEAD", pull.pushed_ref());
        let count = git(&repo.worktree, &["rev-list", "--count", &range])?;
        return (count.trim().parse())
            .map_err(|err| format!("couldn't read a commit count: {err}"));
    }
    let range = format!("{}..HEAD", base_ref(repo));
    let count = git(&repo.worktree, &["rev-list", "--count", &range])?;
    let n: usize = count
        .trim()
        .parse()
        .map_err(|err| format!("couldn't read a commit count: {err}"))?;
    let merged = |branch: &String| merged_into_base(repo, branch) || merged_on_origin(repo, branch);
    Ok(if n > 0 && repo.branch.as_ref().is_some_and(merged) {
        0
    } else {
        n
    })
}

/// Whether `branch` of `repo` is merged into origin's base once that is
/// fetched: a pull request merged on GitHub, its base here not pulled yet.
/// Off the network, it isn't: then the commits count as lost.
fn merged_on_origin(repo: &Repo, branch: &str) -> bool {
    if let Err(failed) = crate::git_ops::fetch_branch(&repo.main, "origin", &repo.base) {
        tracing::info!(
            "fetching {} of {} to count what removing loses: {}",
            repo.base,
            repo.main.display(),
            failed.why
        );
        return false;
    }
    let spec = format!("refs/remotes/origin/{}^{{commit}}", repo.base);
    let Ok(base) = git(&repo.main, &["rev-parse", "--verify", "--quiet", &spec]) else {
        return false;
    };
    in_base(repo, base.trim(), branch)
}

/// Whether `branch` of `repo` is in commit `base`, squashed or not. One
/// fast-forwarded there is too: `git::merged` takes a branch at its target
/// for one with nothing to merge.
fn in_base(repo: &Repo, base: &str, branch: &str) -> bool {
    has_branch(repo, base, branch) || crate::git::merged(&repo.main, branch, &[base])
}

/// Whether `branch` of `repo` is merged into its base, squashed or not.
pub(super) fn merged_into_base(repo: &Repo, branch: &str) -> bool {
    // As a commit: `git::merged` keeps its answers per pair of names.
    let spec = format!("{}^{{commit}}", repo.base);
    let Ok(base) = git(&repo.main, &["rev-parse", "--verify", "--quiet", &spec]) else {
        return false;
    };
    in_base(repo, base.trim(), branch)
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

/// Take `feature`'s worktrees and folders down (every linked project's
/// too), and delete its branches. `force` takes them down even when that
/// loses work (`losses`); without it, a repository that would lose
/// something is left and named in the error. A branch with commits not in
/// its base is kept unless `force`.
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
    for (project, dir) in feature.folders() {
        let repos: Vec<&Repo> = feature.repos_of(project).collect();
        take_down(&feature.branch(), &repos, dir, force, &mut failed, feature.pulls());
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// Each of `feature`'s worktrees that is there, with the commit it is on:
/// what `restore` takes to bring them back once `remove` took them down.
pub fn heads(feature: &Feature) -> Vec<KeptHead> {
    (feature.repos.iter())
        .filter(|r| r.worktree.is_dir())
        .filter_map(|r| {
            let sha = git(&r.worktree, &["rev-parse", "HEAD"]).ok()?;
            Some(KeptHead {
                repo: r.key(),
                sha: sha.trim().to_owned(),
                changes: None,
            })
        })
        .collect()
}

/// The ref keeping `branch`'s head, once abandoned, out of reach of `git gc`.
fn kept_ref(branch: &str) -> String {
    format!("refs/gerdr/kept/{branch}")
}

/// The ref keeping what `branch`'s worktree had uncommitted, once abandoned.
fn kept_changes_ref(branch: &str) -> String {
    format!("refs/gerdr/kept-changes/{branch}")
}

/// `heads` of `feature`, abandoned, with all it has that `remove` would lose
/// kept: each worktree's commit under a ref of its own (its branch goes), and
/// what it has uncommitted, untracked files too, as a stash commit under
/// another, its stash list left as it was. `restore` puts both back. Fails
/// before anything goes when one can't be kept.
pub fn keep(feature: &Feature) -> Result<Vec<KeptHead>, String> {
    let branch = feature.branch();
    let mut heads = heads(feature);
    for i in 0..heads.len() {
        let Some(repo) = feature.repo(&heads[i].repo) else {
            continue;
        };
        let own = repo.branch.as_deref().unwrap_or(&branch);
        match keep_repo(repo, own, &heads[i].sha, &feature.title) {
            Ok(changes) => heads[i].changes = changes,
            Err(why) => {
                // Nothing goes then: the changes kept so far go back.
                unkeep(feature, &heads[..i]);
                return Err(why);
            }
        }
    }
    Ok(heads)
}

/// Put back what `keep` took of `heads` into the worktrees still there:
/// abandoning stopped before they went.
pub fn unkeep(feature: &Feature, heads: &[KeptHead]) {
    for head in heads {
        let (Some(sha), Some(repo)) = (&head.changes, feature.repo(&head.repo)) else {
            continue;
        };
        if repo.worktree.is_dir()
            && let Err(err) = git(&repo.worktree, &["stash", "apply", "-q", "--index", sha])
        {
            tracing::warn!("putting back the changes of {}: {err}", head.repo);
        }
    }
}

/// `keep`'s work on `repo`, whose branch is `branch` at `sha`: its commit
/// under a ref, and what it has uncommitted as a stash commit under another,
/// whose commit it returns. A failure after the stash puts the changes back.
fn keep_repo(repo: &Repo, branch: &str, sha: &str, title: &str) -> Result<Option<String>, String> {
    let name = repo.key();
    git(&repo.main, &["update-ref", &kept_ref(branch), sha])
        .map_err(|err| format!("{name}: couldn't keep its commit: {err}"))?;
    let status = git(&repo.worktree, &["status", "--porcelain", "--untracked-files=all"])
        .map_err(|err| format!("{name}: {err}"))?;
    if status.trim().is_empty() {
        return Ok(None);
    }
    let message = format!("gerdr: {title} abandoned");
    git(&repo.worktree, &["stash", "push", "-q", "--include-untracked", "-m", &message])
        .map_err(|err| format!("{name}: couldn't keep its changes: {err}"))?;
    let kept = git(&repo.worktree, &["rev-parse", "refs/stash"])
        .map(|sha| sha.trim().to_owned())
        .and_then(|sha| {
            git(&repo.main, &["update-ref", &kept_changes_ref(branch), &sha]).map(|_| sha)
        });
    let sha = match kept {
        Ok(sha) => sha,
        Err(err) => {
            if let Err(err) = git(&repo.worktree, &["stash", "pop", "-q", "--index"]) {
                tracing::warn!("putting back the changes of {name}: {err}");
            }
            return Err(format!("{name}: couldn't keep its changes: {err}"));
        }
    };
    // Kept by its own ref now: the user's stash list is as it was.
    if let Err(err) = git(&repo.worktree, &["stash", "drop", "-q"]) {
        tracing::warn!("dropping the stash keeping {name}'s changes: {err}");
    }
    Ok(Some(sha))
}

/// Bring back the worktrees of `feature` that `heads` names, as `remove`
/// took them down: their folders, each worktree at its commit (on the
/// feature's branch again for a repository it changes, made there unless it
/// stayed), and the root's links of a folder holding several. One there
/// already is left as it is. On a failure, what was made goes again before
/// the error returns.
pub fn restore(feature: &Feature, heads: &[KeptHead]) -> Result<(), String> {
    let branch = feature.branch();
    let mut made = Restored::default();
    let result = restore_into(feature, heads, &branch, &mut made);
    if result.is_err() {
        made.undo(&branch);
    }
    result
}

/// What `restore` made, to take down again when it fails.
#[derive(Default)]
struct Restored {
    /// Folders it made, in order.
    dirs: Vec<PathBuf>,
    /// Each worktree it added: its main worktree, its folder, and whether
    /// its branch was made with it.
    worktrees: Vec<(PathBuf, PathBuf, bool)>,
}

impl Restored {
    fn undo(self, branch: &str) {
        for (main, path, branched) in self.worktrees.iter().rev() {
            let p = path.to_string_lossy();
            if let Err(err) = git(main, &["worktree", "remove", "--force", &p]) {
                tracing::warn!("taking down {}: {err}", path.display());
            }
            if *branched && let Err(err) = git(main, &["branch", "-D", branch]) {
                tracing::warn!("deleting {branch} in {}: {err}", main.display());
            }
        }
        for dir in self.dirs.iter().rev() {
            if let Err(err) = std::fs::remove_dir_all(dir)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("removing {}: {err}", dir.display());
            }
        }
    }
}

/// `restore`'s work, noting in `made` what it makes.
fn restore_into(
    feature: &Feature,
    heads: &[KeptHead],
    branch: &str,
    made: &mut Restored,
) -> Result<(), String> {
    for (project, dir) in feature.folders() {
        let (root, multi) = match project {
            None => (feature.root.as_path(), feature.multi),
            Some(id) => match feature.links.iter().find(|l| l.project == id) {
                Some(link) => (link.root.as_path(), link.multi),
                None => continue,
            },
        };
        let repos: Vec<(&Repo, &KeptHead)> = (feature.repos_of(project))
            .filter_map(|r| heads.iter().find(|h| h.repo == r.key()).map(|h| (r, h)))
            .collect();
        if repos.is_empty() {
            continue;
        }
        // `.features`, then the feature's folder when it holds several.
        let new_dir = dir.symlink_metadata().is_err();
        let mut wanted: Vec<&Path> = dir.parent().into_iter().collect();
        if multi {
            wanted.push(dir);
        }
        for folder in wanted {
            if folder.symlink_metadata().is_ok() {
                continue;
            }
            std::fs::create_dir(folder)
                .map_err(|err| format!("couldn't make {}: {err}", shown(folder)))?;
            made.dirs.push(folder.to_path_buf());
        }
        for (repo, head) in repos {
            if repo.worktree.symlink_metadata().is_err() {
                restore_worktree(repo, head, branch, made)?;
            }
        }
        if multi && new_dir {
            link_root(root, dir)?;
        }
    }
    Ok(())
}

/// Add `repo`'s worktree back at `head`'s commit: on `branch` if the
/// feature changes it, detached for a context repository.
fn restore_worktree(
    repo: &Repo,
    head: &KeptHead,
    branch: &str,
    made: &mut Restored,
) -> Result<(), String> {
    let name = repo.key();
    let short: String = head.sha.chars().take(7).collect();
    let spec = format!("{}^{{commit}}", head.sha);
    if git(&repo.main, &["cat-file", "-e", &spec]).is_err() {
        return Err(format!("{name}: its commit {short} is gone"));
    }
    // A worktree that went by hand leaves its record behind.
    if let Err(err) = git(&repo.main, &["worktree", "prune"]) {
        tracing::warn!("pruning worktrees of {name}: {err}");
    }
    let p = repo.worktree.to_string_lossy();
    let add = |args: &[&str]| {
        let mut all = vec!["worktree", "add", "-q"];
        all.extend_from_slice(args);
        git(&repo.main, &all).map_err(|err| format!("{name}: {err}"))
    };
    let branched = if !repo.touched {
        add(&["--detach", &p, &head.sha])?;
        false
    } else if branch_exists(&repo.main, branch) {
        // Kept when it went (it had commits of its own): it must be where
        // it was.
        let full = format!("refs/heads/{branch}");
        let at = git(&repo.main, &["rev-parse", &full]).map_err(|err| format!("{name}: {err}"))?;
        if at.trim() != head.sha {
            return Err(format!("{name}: {branch} is there again, not at {short}"));
        }
        add(&[&p, branch])?;
        false
    } else {
        add(&["-b", branch, &p, &head.sha])?;
        true
    };
    made.worktrees
        .push((repo.main.clone(), repo.worktree.clone(), branched));
    if let Some(changes) = &head.changes {
        git(
            &repo.worktree,
            &["stash", "apply", "-q", "--index", changes],
        )
        .map_err(|err| format!("{name}: couldn't put its changes back: {err}"))?;
    }
    if repo.touched {
        guard_worktree(repo, branch);
    }
    // What kept it, once abandoned, has done its job.
    let own = repo.branch.as_deref().unwrap_or(branch);
    for name in [kept_ref(own), kept_changes_ref(own)] {
        if git(&repo.main, &["rev-parse", "-q", "--verify", &name]).is_ok()
            && let Err(err) = git(&repo.main, &["update-ref", "-d", &name])
        {
            tracing::warn!("dropping {name}: {err}");
        }
    }
    Ok(())
}

/// Why repositories `keys` can't leave `feature`, whatever they hold: the
/// home project's single repository is the feature's folder, and a feature
/// has to change one.
pub fn removal_refusal(feature: &Feature, keys: &[String]) -> Option<String> {
    let goes = |r: &Repo| keys.contains(&r.key());
    if !feature.multi
        && let Some(home) = feature.repos_of(None).find(|r| goes(r))
    {
        return Some(format!("{} is the feature's own folder", home.name));
    }
    let left = feature.touched().any(|r| !goes(r));
    let any = feature.touched().any(goes);
    (any && !left).then(|| "a feature needs a repository to change".to_owned())
}

/// What taking repositories `keys` out of `feature` would lose, as `losses`
/// says it; an error when one has commits not in its base, which stop it:
/// they are the feature's work, and shipping them is the way out.
pub fn removal_losses(feature: &Feature, keys: &[String]) -> Result<Vec<String>, String> {
    let repos: Vec<&Repo> = (feature.repos.iter())
        .filter(|r| keys.contains(&r.key()))
        .collect();
    for repo in &repos {
        if !repo.touched || !repo.worktree.is_dir() {
            continue;
        }
        let n = commits_of(repo, feature.pull_of(&repo.key())).map_err(|err| format!("{}: {err}", repo.key()))?;
        if n > 0 {
            return Err(format!(
                "{} has {n} {} not in {}",
                repo.key(),
                plural(n, "commit"),
                kept_in(repo, feature.pull_of(&repo.key()))
            ));
        }
    }
    Ok(repos.into_iter().filter_map(|r| loss(r, feature.pull_of(&r.key()))).collect())
}

/// Take repositories `keys` out of `feature`: their worktrees and branches
/// go, and a linked project's folder once none of its repositories are
/// left. The home folder stays. Without `force`, one that would lose work
/// is left and says so. Says for each key how it went; the caller drops
/// those gone from the record, and a link with none left.
pub fn remove_repos(
    feature: &Feature,
    keys: &[String],
    force: bool,
) -> Vec<(String, Result<(), String>)> {
    let branch = feature.branch();
    let mut results: Vec<(String, Result<(), String>)> = Vec::new();
    for key in keys {
        let result = match feature.repo(key) {
            None => Err(format!("{} has no repository {key}", feature.title)),
            Some(repo) => match loss(repo, feature.pull_of(&repo.key())).filter(|_| !force) {
                Some(lost) => Err(format!("it would lose work, so it was left: {lost}")),
                None => {
                    let mut failed = Vec::new();
                    take_down_repo(&branch, repo, force, &mut failed, feature.pull_of(&repo.key()));
                    if failed.is_empty() {
                        Ok(())
                    } else {
                        Err(failed.join("; "))
                    }
                }
            },
        };
        results.push((key.clone(), result));
    }
    for link in &feature.links {
        let id = Some(link.project.as_str());
        let gone = |r: &Repo| {
            let key = r.key();
            results
                .iter()
                .any(|(k, result)| *k == key && result.is_ok())
        };
        // One without any was never this call's to take down.
        if !feature.repos_of(id).any(gone) || !feature.repos_of(id).all(gone) {
            continue;
        }
        let mut failed = Vec::new();
        clear_folder(&link.dir, force, &mut failed);
        if !failed.is_empty() {
            // The repositories are gone either way: what stays is a folder.
            tracing::warn!("taking {} down: {}", shown(&link.dir), failed.join("; "));
        }
    }
    results
}

/// Take down `repos`' worktrees of a feature on `branch`, all in feature
/// folder `dir`, then `dir` once they are gone, noting what failed in
/// `failed`.
fn take_down(
    branch: &str,
    repos: &[&Repo],
    dir: &Path,
    force: bool,
    failed: &mut Vec<String>,
    pulls: &[PullRef],
) {
    let mut left = false;
    for repo in repos {
        let pull = pulls.iter().find(|p| p.repo == repo.key() || p.repo.is_empty());
        left |= !take_down_repo(branch, repo, force, failed, pull);
    }
    if !left {
        clear_folder(dir, force, failed);
    } else {
        clear_features(dir);
    }
}

/// Take down `repo`'s worktree of a feature on `branch` and that branch,
/// noting what failed in `failed`. Whether the worktree went. The review of
/// the user's own pull request `pull` leaves its branch: it is theirs.
fn take_down_repo(
    branch: &str,
    repo: &Repo,
    force: bool,
    failed: &mut Vec<String>,
    pull: Option<&PullRef>,
) -> bool {
    // Each repository is on its own: a review's pull requests' differ.
    let branch = repo.branch.as_deref().unwrap_or(branch);
    let name = repo.key();
    if repo.worktree.symlink_metadata().is_ok() {
        let p = repo.worktree.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&p);
        if let Err(err) = git(&repo.main, &args) {
            failed.push(format!("{name}: {err}"));
            return false;
        }
    } else if let Err(err) = git(&repo.main, &["worktree", "prune"]) {
        failed.push(format!("{name}: {err}"));
    }
    if !repo.touched {
        return true;
    }
    if let Err(err) = hook::unguard(&repo.main, branch) {
        tracing::warn!("unguarding {branch} of {name}: {err}");
    }
    // What its head was fetched into goes with it, once the branch was
    // checked against it: a branch kept keeps it.
    let drop_fetched = || {
        if let Some(pull) = pull
            && let Err(err) = git(&repo.main, &["update-ref", "-d", &pull.fetch_ref()])
        {
            tracing::warn!("removing {} of {name}: {err}", pull.fetch_ref());
        }
    };
    if !branch_exists(&repo.main, branch) || pull.is_some_and(PullRef::own) {
        drop_fetched();
        return true;
    }
    if !force {
        let from = pull.map_or_else(|| base_ref(repo), PullRef::pushed_ref);
        let range = format!("{from}..{branch}");
        let ahead = git(&repo.main, &["rev-list", "--count", &range])
            .ok()
            .and_then(|n| n.trim().parse::<usize>().ok());
        if ahead != Some(0) && (pull.is_some() || !merged_into_base(repo, branch)) {
            failed.push(format!(
                "{name}: kept branch {branch}, it has commits not in {}",
                kept_in(repo, pull)
            ));
            return true;
        }
    }
    if let Err(err) = git(&repo.main, &["branch", "-D", branch]) {
        failed.push(format!("{name}: {err}"));
    }
    drop_fetched();
    true
}

/// Remove feature folder `dir`, its worktrees gone, noting a failure in
/// `failed`; then `.features` if that leaves it empty.
fn clear_folder(dir: &Path, force: bool, failed: &mut Vec<String>) {
    if dir.symlink_metadata().is_ok()
        && let Err(err) = clear_dir(dir, force)
    {
        failed.push(err);
    }
    clear_features(dir);
}

/// Remove the `.features` folder holding `dir`, if it is empty.
fn clear_features(dir: &Path) {
    if let Some(features) = dir.parent()
        && features.file_name().is_some_and(|n| n == FEATURES_DIR)
    {
        // Only when empty: other features may be in it.
        let _ = std::fs::remove_dir(features);
    }
}

/// Remove a feature folder whose worktrees are gone: its links, then the
/// folder; anything else in it only with `force`. Its code graph is no
/// work to lose: it is built again from the code.
fn clear_dir(dir: &Path, force: bool) -> Result<(), String> {
    if force {
        return std::fs::remove_dir_all(dir)
            .map_err(|err| format!("couldn't remove {}: {err}", shown(dir)));
    }
    let graph = dir.join(super::graph::OUT_DIR);
    if graph.symlink_metadata().is_ok()
        && let Err(err) = std::fs::remove_dir_all(&graph)
    {
        tracing::warn!("removing {}: {err}", graph.display());
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
    let mut cmd = git_command(dir, args);
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
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
    match out.status.success() {
        true => Ok(out.stdout),
        false => Err(why_failed(args, &out.stderr)),
    }
}

/// `git`, stopped with what it started after `limit`: one that goes over
/// the network.
fn git_within(dir: &Path, args: &[&str], limit: Duration) -> Result<String, String> {
    let out = crate::t3_update::output_within(git_command(dir, args), limit)
        .map_err(|err| format!("git {}: {err}", args.first().unwrap_or(&"")))?;
    match out.status.success() {
        true => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        false => Err(why_failed(args, &out.stderr)),
    }
}

/// A git of `args` in `dir`, on its own settings: never prompting, nothing
/// of the git that runs gerdr's.
fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        // A probe's `git status` must never take index.lock from an agent.
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

/// Why a git of `args` failed, from what it printed on `stderr`.
fn why_failed(args: &[&str], stderr: &[u8]) -> String {
    let err = String::from_utf8_lossy(stderr);
    // The last line says why; the ones before it are hints.
    let why = err
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("hint:"))
        .unwrap_or("")
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ");
    if why.is_empty() {
        format!("git {} failed", args.first().unwrap_or(&""))
    } else {
        why.to_owned()
    }
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

    /// A clone `app` of a repository `origin` in `t` whose pull request 7
    /// is branch `fix` with one commit: the commit.
    fn pull_seed(t: &Temp) -> (PathBuf, String) {
        let origin = t.repo("origin", "main");
        run(&origin, &["checkout", "-q", "-b", "fix"]);
        commit(&origin, "b", "the fix");
        let sha = run(&origin, &["rev-parse", "HEAD"]).trim().to_owned();
        run(&origin, &["update-ref", "refs/pull/7/head", &sha]);
        run(&origin, &["checkout", "-q", "main"]);
        run(&t.0, &["clone", "-q", "origin", "app"]);
        (t.0.join("app"), sha)
    }

    fn pull_of(sha: &str, mine: bool) -> PullRef {
        PullRef {
            host: "github.com".into(),
            slug: "o/app".into(),
            number: 7,
            url: "https://github.com/o/app/pull/7".into(),
            mine,
            base_ref: "main".into(),
            head_ref: "fix".into(),
            head_sha: sha.into(),
            ..PullRef::default()
        }
    }

    #[test]
    fn a_fetch_that_hangs_is_stopped() {
        let t = Temp::new();
        let (app, _) = pull_seed(&t);
        run(&app, &["config", "remote.origin.uploadpack", "sleep 30; git-upload-pack"]);
        let start = std::time::Instant::now();
        let stopped = git_within(&app, &["fetch", "-q", "origin"], Duration::from_secs(1));
        assert_eq!(stopped, Err("git fetch: no answer in 1s".into()));
        assert!(start.elapsed() < Duration::from_secs(10));
        // Answering, it fetches as `git` does.
        run(&app, &["config", "--unset", "remote.origin.uploadpack"]);
        assert_eq!(git_within(&app, &["fetch", "-q", "origin"], FETCH_TIME), Ok(String::new()));
        assert!(git_within(&app, &["fetch", "-q", "nowhere"], FETCH_TIME).is_err());
    }

    #[test]
    fn reviews_someone_elses_pull_request() {
        let t = Temp::new();
        let (app, sha) = pull_seed(&t);
        let root = find_root(&app, &[], &[], &[]).unwrap();
        let pull = pull_of(&sha, false);
        let created = create_pr(&root, "app", &pull, "pr-7").unwrap();
        let repo = &created.repos[0];
        assert_eq!(repo.worktree, app.join(".features/pr-7"));
        assert_eq!(run(&repo.worktree, &["rev-parse", "HEAD"]).trim(), sha);
        assert_eq!(repo.branch.as_deref(), Some("gerdr/pr-7"));
        assert_eq!(repo.base, "origin/main");
        assert!(hook::guarded(&app, "gerdr/pr-7"));
        let details = super::super::pr::Details {
            pull: pull.clone(),
            title: "Fix".into(),
            state: "OPEN".into(),
            ..Default::default()
        };
        let feature =
            super::super::pr::record("p", &root, created, std::slice::from_ref(&details), None, 1);
        // Its commits are on GitHub: nothing is lost.
        assert_eq!(losses(&feature), Vec::<String>::new());
        commit(&feature.repos[0].worktree, "c", "mine");
        let lost = losses(&feature);
        assert!(lost[0].contains("1 commit not in #7"), "{lost:?}");
        assert_eq!(create_pr(&root, "app", &pull, "pr-7").map(|_| ()), Err(format!(
            "{} already exists",
            shown(&app.join(".features/pr-7"))
        )));
        remove(&feature, true).unwrap();
        assert!(!branch_exists(&app, "gerdr/pr-7"));
        assert!(git(&app, &["rev-parse", "--verify", "-q", "refs/gerdr/pr/7"]).is_err());
    }

    #[test]
    fn reviews_a_pull_request_per_repository_and_takes_back_a_failure() {
        let t = Temp::new();
        std::fs::create_dir_all(t.0.join("work")).unwrap();
        let seed = |name: &str, n: u64| {
            let origin = t.repo(&format!("{name}-origin"), "main");
            run(&origin, &["checkout", "-q", "-b", "fix"]);
            commit(&origin, "b", "the fix");
            let sha = run(&origin, &["rev-parse", "HEAD"]).trim().to_owned();
            run(&origin, &["update-ref", &format!("refs/pull/{n}/head"), &sha]);
            run(&origin, &["checkout", "-q", "main"]);
            let to = format!("work/{name}");
            run(&t.0, &["clone", "-q", &format!("{name}-origin"), &to]);
            let mut pull = pull_of(&sha, false);
            pull.number = n;
            pull.repo = name.into();
            pull
        };
        let (api, web) = (seed("api", 7), seed("web", 8));
        let dirs = [t.0.join("work/api"), t.0.join("work/web")];
        let root = find_root(&dirs[0], &dirs, &[], &[]).unwrap();
        assert!(root.multi);
        let picks = [("api".to_owned(), api.clone()), ("web".to_owned(), web.clone())];
        let (created, linked) = create_review(&root, &picks, &[], "pr-fix").unwrap();
        assert!(linked.is_empty());
        let names: Vec<(&str, Option<&str>)> = (created.repos.iter())
            .map(|r| (r.name.as_str(), r.branch.as_deref()))
            .collect();
        assert_eq!(names, vec![("api", Some("gerdr/pr-7")), ("web", Some("gerdr/pr-8"))]);
        let head = |dir: &Path| run(dir, &["rev-parse", "HEAD"]).trim().to_owned();
        assert_eq!(head(&t.0.join("work/.features/pr-fix/web")), web.head_sha);
        assert!(hook::guarded(&dirs[1], "gerdr/pr-8"));
        undo_review(&created, &[api.clone(), web.clone()]);
        assert!(!branch_exists(&dirs[1], "gerdr/pr-8"));
        // A linked project's that can't be fetched takes the home one's back.
        let alone = find_root(&dirs[0], &[], &[], &[]).unwrap();
        let other = find_root(&dirs[1], &[], &[], &[]).unwrap();
        let mut gone = web.clone();
        gone.number = 9;
        let links = [LinkedPicks {
            spec: LinkSpec {
                project: "other".into(),
                name: "Other".into(),
                root: other,
                touched: Vec::new(),
            },
            picks: vec![("web".into(), gone)],
        }];
        let failed = create_review(&alone, &[("api".into(), api)], &links, "pr-again");
        assert!(failed.as_ref().is_err_and(|e| e.starts_with("Other: web: ")), "{failed:?}");
        assert!(!dirs[0].join(".features/pr-again").exists());
        assert!(!branch_exists(&dirs[0], "gerdr/pr-7"));
    }

    #[test]
    fn adds_a_pull_request_to_an_open_review() {
        let t = Temp::new();
        std::fs::create_dir_all(t.0.join("work")).unwrap();
        let seed = |name: &str, n: u64| {
            let origin = t.repo(&format!("{name}-origin"), "main");
            run(&origin, &["checkout", "-q", "-b", "fix"]);
            commit(&origin, "b", "the fix");
            let sha = run(&origin, &["rev-parse", "HEAD"]).trim().to_owned();
            run(&origin, &["update-ref", &format!("refs/pull/{n}/head"), &sha]);
            run(&origin, &["checkout", "-q", "main"]);
            let to = format!("work/{name}");
            run(&t.0, &["clone", "-q", &format!("{name}-origin"), &to]);
            let mut pull = pull_of(&sha, false);
            pull.number = n;
            pull.repo = name.into();
            pull
        };
        let (api, web) = (seed("api", 7), seed("web", 8));
        let dirs = [t.0.join("work/api"), t.0.join("work/web")];
        let root = find_root(&dirs[0], &dirs, &[], &[]).unwrap();
        let (created, _) = create_review(&root, &[("api".into(), api.clone())], &[], "pr-7").unwrap();
        let details = super::super::pr::Details {
            pull: api.clone(),
            state: "OPEN".into(),
            ..Default::default()
        };
        let feature =
            super::super::pr::record("p", &root, created, std::slice::from_ref(&details), None, 1);
        let add = |pull: &PullRef| PrAdd {
            project: None,
            name: "p".into(),
            root: root.clone(),
            picks: vec![("web".into(), pull.clone())],
        };
        // One that can't be fetched leaves nothing behind.
        let mut gone = web.clone();
        gone.number = 9;
        assert!(add_review(&feature, &[add(&gone)]).is_err());
        assert!(!feature.dir.join("web").exists());
        let (linked, repos) = add_review(&feature, &[add(&web)]).unwrap();
        let mut whole = feature.clone();
        whole.repos.extend(repos.clone());
        if let super::super::Kind::PrReview(pulls) = &mut whole.kind {
            pulls.push(web.clone());
        }
        assert!(linked.is_empty());
        assert_eq!(repos[0].worktree, feature.dir.join("web"));
        assert_eq!(repos[0].branch.as_deref(), Some("gerdr/pr-8"));
        let head = run(&repos[0].worktree, &["rev-parse", "HEAD"]);
        assert_eq!(head.trim(), web.head_sha);
        // Twice is refused: it is there.
        let again = add_review(&feature, &[add(&web)]).unwrap_err();
        assert!(again.contains("already has web"), "{again}");
        // Left out with nothing of the user's on it: its branch and the
        // head fetched for it go, unforced.
        let gone = remove_repos(&whole, &["web".to_owned()], false);
        assert_eq!(gone, vec![("web".to_owned(), Ok(()))]);
        assert!(!branch_exists(&dirs[1], "gerdr/pr-8"));
        assert!(git(&dirs[1], &["rev-parse", "--verify", "-q", "refs/gerdr/pr/8"]).is_err());
        assert!(branch_exists(&dirs[0], "gerdr/pr-7"));
    }

    #[test]
    fn a_review_moves_to_new_commits_and_keeps_yours() {
        let t = Temp::new();
        let (app, sha) = pull_seed(&t);
        let root = find_root(&app, &[], &[], &[]).unwrap();
        let pull = pull_of(&sha, false);
        let created = create_pr(&root, "app", &pull, "pr-7").unwrap();
        let repo = created.repos[0].clone();
        // The author pushes a fix, rewriting what came before.
        let origin = t.0.join("origin");
        run(&origin, &["checkout", "-q", "fix"]);
        run(&origin, &["commit", "-q", "--amend", "-m", "the fix, again"]);
        commit(&origin, "c", "more");
        let new = run(&origin, &["rev-parse", "HEAD"]).trim().to_owned();
        run(&origin, &["update-ref", "refs/pull/7/head", &new]);
        run(&origin, &["checkout", "-q", "main"]);
        let to = super::super::pr::Remote {
            at: 1,
            head_sha: new.clone(),
            base_ref: "main".into(),
            state: "OPEN".into(),
            checks: Vec::new(),
        };
        // Changes of the user's hold it.
        std::fs::write(repo.worktree.join("a"), "edited").unwrap();
        let held = update_pr(&repo, &pull, &to).unwrap_err();
        assert!(held.contains("uncommitted changes"), "{held}");
        run(&repo.worktree, &["checkout", "-q", "--", "a"]);
        let moved = update_pr(&repo, &pull, &to).unwrap();
        assert_eq!(moved.head_sha, new);
        assert_eq!(run(&repo.worktree, &["rev-parse", "HEAD"]).trim(), new);
        assert_eq!(moved.base, "origin/main");
        // A commit of the user's on it holds the next move.
        let mut on = pull.clone();
        on.moved_to(&new, "main");
        commit(&repo.worktree, "d", "mine");
        let held = update_pr(&repo, &on, &to).unwrap_err();
        assert!(held.contains("1 commit of yours not in #7"), "{held}");
    }

    #[test]
    fn reviews_own_pull_request_on_its_branch() {
        let t = Temp::new();
        let (app, sha) = pull_seed(&t);
        let root = find_root(&app, &[], &[], &[]).unwrap();
        let pull = pull_of(&sha, true);
        let created = create_pr(&root, "app", &pull, "pr-7").unwrap();
        let repo = &created.repos[0];
        assert_eq!(repo.branch.as_deref(), Some("fix"));
        assert_eq!(run(&repo.worktree, &["rev-parse", "HEAD"]).trim(), sha);
        // The user pushes it, from anywhere.
        assert!(!hook::guarded(&app, "fix"));
        let details = super::super::pr::Details {
            pull,
            state: "OPEN".into(),
            ..Default::default()
        };
        let feature =
            super::super::pr::record("p", &root, created, std::slice::from_ref(&details), None, 1);
        remove(&feature, true).unwrap();
        // Its branch is the user's: it stays.
        assert!(branch_exists(&app, "fix"));
    }

    fn feature_of(root: &Root, created: Created, slug: &str) -> Feature {
        Feature {
            version: VERSION,
            kind: Default::default(),
            writer: None,
            slug: slug.into(),
            title: slug.into(),
            project: "p".into(),
            created: 1,
            root: root.path.clone(),
            dir: created.dir,
            multi: root.multi,
            repos: created.repos,
            links: Vec::new(),
            status: Status::Open,
            marks: Default::default(),
            run_counts: Default::default(),
            context: Vec::new(),
            removed: Vec::new(),
            shipping: Default::default(),
            ship_plan: Default::default(),
            runs: Default::default(),
            stage_agents: Default::default(),
            stage_machines: Default::default(),
            setups_elsewhere: Vec::new(),
            came_from: None,
            stage_models: Default::default(),
            fresh_milestones: false,
            setup: None,
            checks: Default::default(),
            skip_checks: Default::default(),
            verify: None,
            sandbox: None,
            sandbox_picks: Default::default(),
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
            fix_round: None,
            last_fix_round: None,
            verify_fixes: Vec::new(),
            fix_asked: Default::default(),
            comment_fixes: Vec::new(),
            ran_once: Default::default(),
            fix_rounds: 0,
            past_rounds: Vec::new(),
            drafts: Vec::new(),
            carry: None,
            unverified: None,
            back_from: None,
            epoch: 0,
            history: Vec::new(),
        }
    }

    /// `main`'s repository as a feature made before linking holds one left
    /// out of it: detached at its base in worktree `path`.
    fn context_repo(main: &Path, path: &Path) -> Repo {
        run(
            main,
            &["worktree", "add", "-q", "--detach", path.to_str().unwrap()],
        );
        Repo {
            name: folder_name(main),
            main: main.to_path_buf(),
            base: "main".into(),
            base_commit: run(main, &["rev-parse", "main"]).trim().into(),
            worktree: path.to_path_buf(),
            touched: false,
            branch: None,
            project: None,
        }
    }

    fn names(root: &Root) -> Vec<&str> {
        root.repos.iter().map(|r| r.name.as_str()).collect()
    }

    /// A feature of a single repository with its docs linked to `data`.
    fn with_docs(t: &Temp) -> (PathBuf, Created, PathBuf, Root) {
        let repo = t.repo("app", "main");
        let root = find_root(&repo, std::slice::from_ref(&repo), &[], &[]).unwrap();
        let created = create(&root, "login", &[]).unwrap();
        let data = t.0.join("data");
        link_docs(&created.dir, false, &data).unwrap();
        std::fs::write(data.join("brief.md"), "the brief").unwrap();
        (repo, created, data, root)
    }

    #[test]
    fn git_never_sees_the_docs_link_of_a_single_repository() {
        let t = Temp::new();
        let (repo, created, data, root) = with_docs(&t);
        let dir = &created.dir;
        assert_eq!(
            std::fs::read_to_string(dir.join(DOCS_LINK).join("brief.md")).unwrap(),
            "the brief"
        );
        assert_eq!(run(dir, &["status", "--porcelain"]), "");
        run(dir, &["add", "-A"]);
        assert_eq!(run(dir, &["diff", "--cached", "--name-only"]), "");
        let others = ["ls-files", "--others", "--exclude-standard"];
        assert_eq!(run(dir, &others), "");
        // Nor does the main worktree, and removing the feature loses nothing.
        assert_eq!(run(&repo, &["status", "--porcelain"]), "");
        let feature = feature_of(&root, created.clone(), "login");
        assert!(losses(&feature).is_empty());
        // No trailing `/`: git takes a symlink for a file.
        let exclude = repo.join(".git/info/exclude");
        let text = std::fs::read_to_string(exclude).unwrap();
        assert!(text.lines().any(|l| l == "/feature-docs"), "{text}");
        assert!(data.join("brief.md").is_file());
    }

    #[test]
    fn the_docs_link_is_no_part_of_a_features_patch() {
        let t = Temp::new();
        let (_repo, created, data, root) = with_docs(&t);
        let dir = created.dir.clone();
        commit(&dir, "b", "second");
        let feature = feature_of(&root, created, "login");
        let with_link = crate::feature::patch::read(&feature).unwrap();
        std::fs::remove_file(dir.join(DOCS_LINK)).unwrap();
        let without = crate::feature::patch::read(&feature).unwrap();
        // When it was read says nothing of the code.
        assert_eq!(
            crate::feature::patch::Reading {
                at: with_link.at,
                ..without
            },
            with_link
        );
        assert!(data.join("brief.md").is_file());
    }

    #[test]
    fn taking_down_a_feature_leaves_its_docs() {
        let t = Temp::new();
        let (_repo, created, data, root) = with_docs(&t);
        let dir = created.dir.clone();
        let feature = feature_of(&root, created, "login");
        let result = remove(&feature, false);
        assert!(result.is_ok(), "{result:?}");
        assert!(dir.symlink_metadata().is_err());
        assert_eq!(
            std::fs::read_to_string(data.join("brief.md")).unwrap(),
            "the brief"
        );
    }

    #[test]
    fn a_docs_link_in_a_multi_repository_folder_goes_with_it() {
        let t = Temp::new();
        let (a, b) = (t.repo("a", "main"), t.repo("b", "main"));
        let root = find_root(&t.0, &[a.clone(), b], &[], &[]).unwrap();
        assert!(root.multi);
        let created = create(&root, "login", &["a".to_owned()]).unwrap();
        let dir = created.dir.clone();
        let data = t.0.join("data");
        assert_eq!(link_docs(&dir, true, &data).unwrap(), None);
        std::fs::write(data.join("brief.md"), "the brief").unwrap();
        assert!(dir.join(DOCS_LINK).join("brief.md").is_file());
        // Not taken for a repository, wherever repositories are looked for.
        assert_eq!(child_repos(&dir), []);
        assert_eq!(names(&find_root(&dir, &[], &[], &[]).unwrap()), ["a", "b"]);
        // Nothing of a repository was written to.
        assert_eq!(run(&a, &["status", "--porcelain"]), "");
        let text = std::fs::read_to_string(a.join(".git/info/exclude")).unwrap_or_default();
        assert!(!text.contains(DOCS_LINK), "{text}");

        let feature = feature_of(&root, created, "login");
        let result = remove(&feature, false);
        assert!(result.is_ok(), "{result:?}");
        assert!(dir.symlink_metadata().is_err());
        assert!(data.join("brief.md").is_file());
    }

    #[test]
    fn linking_docs_twice_changes_nothing() {
        let t = Temp::new();
        let (repo, created, data, _) = with_docs(&t);
        let exclude = repo.join(".git/info/exclude");
        let before = std::fs::read_to_string(&exclude).unwrap();
        assert_eq!(link_docs(&created.dir, false, &data).unwrap(), None);
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), before);
        assert_eq!(
            std::fs::read_link(created.dir.join(DOCS_LINK)).unwrap(),
            data
        );
    }

    #[test]
    fn linking_docs_says_the_exclude_it_changed_and_makes_their_folder() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let root = find_root(&repo, std::slice::from_ref(&repo), &[], &[]).unwrap();
        let created = create(&root, "login", &[]).unwrap();
        let data = t.0.join("not/yet");
        let changed = link_docs(&created.dir, false, &data).unwrap();
        assert_eq!(changed, Some(repo.join(".git/info/exclude")));
        assert!(data.is_dir());
    }

    #[test]
    fn something_else_of_the_docs_links_name_is_left_alone() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        std::fs::create_dir(repo.join(DOCS_LINK)).unwrap();
        commit(&repo, "feature-docs/own.md", "own");
        let root = find_root(&repo, std::slice::from_ref(&repo), &[], &[]).unwrap();
        let created = create(&root, "login", &[]).unwrap();
        let err = link_docs(&created.dir, false, &t.0.join("data")).unwrap_err();
        assert!(err.contains("feature-docs"), "{err}");
        assert!(created.dir.join("feature-docs/own.md").is_file());
        // Refused before git was told to look away.
        let text = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert!(!text.contains(DOCS_LINK), "{text}");
    }

    #[test]
    fn a_folder_gone_has_no_docs_link() {
        let t = Temp::new();
        let data = t.0.join("data");
        let gone = t.0.join("gone");
        assert_eq!(link_docs(&gone, false, &data).unwrap(), None);
        assert!(!data.exists());
    }

    #[test]
    fn a_line_for_a_link_is_not_covered_by_one_for_folders() {
        let added = add_line("feature-docs/\n", "/feature-docs").unwrap();
        assert_eq!(added, "feature-docs/\n/feature-docs\n");
        for has in ["feature-docs", "/feature-docs", "**/feature-docs"] {
            assert_eq!(add_line(has, "/feature-docs"), None, "{has}");
        }
        for has in [".features", ".features/", "/.features/", "**/.features"] {
            assert_eq!(add_line(has, "/.features/"), None, "{has}");
        }
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
    fn taking_a_feature_down_leaves_its_context_repositories_alone() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let docs = t.repo("docs", "main");
        std::fs::write(docs.join("draft"), "kept").unwrap();
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &[]).unwrap();
        let mut f = feature_of(&root, created, "x");
        f.add_context(super::super::ContextRepo {
            name: "docs".into(),
            main: docs.clone(),
            project: None,
        });
        remove(&f, true).unwrap();
        assert_eq!(std::fs::read_to_string(docs.join("draft")).unwrap(), "kept");
        assert!(docs.join(".git").exists());
    }

    #[test]
    fn removed_worktrees_come_back_where_they_were() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &[]).unwrap();
        let dir = created.dir.clone();
        let f = feature_of(&root, created, "x");
        commit(&dir, "b", "second");
        let heads = heads(&f);
        let head = run(&dir, &["rev-parse", "HEAD"]).trim().to_owned();
        assert_eq!(
            heads,
            [KeptHead {
                repo: "app".into(),
                sha: head.clone(),
                changes: None,
            }]
        );
        remove(&f, true).unwrap();
        assert!(!dir.exists());

        restore(&f, &heads).unwrap();
        assert_eq!(run(&dir, &["rev-parse", "HEAD"]).trim(), head);
        assert_eq!(run(&dir, &["branch", "--show-current"]).trim(), "feature/x");
        assert!(hook::guarded(&repo, "feature/x"));
        // Again: there already, it is left.
        restore(&f, &heads).unwrap();

        // A branch made again since, elsewhere, stops it, and nothing stays.
        remove(&f, true).unwrap();
        run(&repo, &["branch", "feature/x", "main"]);
        let err = restore(&f, &heads).unwrap_err();
        assert!(err.contains("feature/x is there again"), "{err}");
        assert!(!repo.join(".features").exists());
        assert!(branch_exists(&repo, "feature/x"));
        // A commit git let go of can't come back.
        run(&repo, &["branch", "-D", "feature/x"]);
        let gone = [KeptHead {
            repo: "app".into(),
            sha: "0".repeat(40),
            changes: None,
        }];
        let err = restore(&f, &gone).unwrap_err();
        assert!(err.contains("is gone"), "{err}");
    }

    #[test]
    fn an_abandoned_feature_comes_back_with_its_commits_and_changes() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &[]).unwrap();
        let dir = created.dir.clone();
        let f = feature_of(&root, created, "x");
        commit(&dir, "b", "second");
        let head = run(&dir, &["rev-parse", "HEAD"]).trim().to_owned();
        std::fs::write(dir.join("b"), "changed").unwrap();
        std::fs::write(dir.join("new"), "untracked").unwrap();
        let stashes = run(&repo, &["stash", "list"]);

        let kept = keep(&f).unwrap();
        assert_eq!(kept[0].sha, head);
        assert!(kept[0].changes.is_some());
        assert_eq!(run(&repo, &["stash", "list"]), stashes);
        remove(&f, true).unwrap();
        assert!(!branch_exists(&repo, "feature/x"));
        // Its commit and changes outlive gc: refs keep them.
        run(&repo, &["gc", "-q", "--prune=now"]);

        restore(&f, &kept).unwrap();
        assert_eq!(run(&dir, &["rev-parse", "HEAD"]).trim(), head);
        assert_eq!(std::fs::read_to_string(dir.join("b")).unwrap(), "changed");
        assert_eq!(
            std::fs::read_to_string(dir.join("new")).unwrap(),
            "untracked"
        );
        let refs = run(&repo, &["for-each-ref", "refs/gerdr"]);
        assert_eq!(refs.trim(), "", "kept refs go once restored");
    }

    #[test]
    fn a_multi_repository_folder_comes_back_with_its_links() {
        let t = Temp::new();
        let api = t.repo("api", "main");
        let web = t.repo("web", "main");
        std::fs::write(t.0.join("AGENTS.md"), "x").unwrap();
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "auth", &["web".into(), "api".into()]).unwrap();
        let dir = created.dir.clone();
        let f = feature_of(&root, created, "auth");
        let heads = heads(&f);
        assert_eq!(heads.len(), 2);
        remove(&f, true).unwrap();
        assert!(!t.0.join(".features").exists());

        restore(&f, &heads).unwrap();
        for (repo, main) in [("api", &api), ("web", &web)] {
            assert!(branch_exists(main, "feature/auth"), "{repo}");
            assert_eq!(
                run(&dir.join(repo), &["branch", "--show-current"]).trim(),
                "feature/auth"
            );
        }
        assert!(
            dir.join("AGENTS.md")
                .symlink_metadata()
                .unwrap()
                .is_symlink()
        );
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
    fn a_linked_project_gets_a_folder_of_its_own_and_goes_with_the_feature() {
        let t = Temp::new();
        let home = t.repo("app", "main");
        let other = t.0.join("other");
        std::fs::create_dir(&other).unwrap();
        let lib = other.join("lib");
        std::fs::create_dir(&lib).unwrap();
        run(&lib, &["init", "-q", "-b", "main"]);
        commit(&lib, "a", "first");
        let root = find_root(&home, &[], &[], &[]).unwrap();
        let mut f = feature_of(&root, create(&root, "x", &[]).unwrap(), "x");
        let linked = find_root(&lib, &[], &[], &[]).unwrap();
        assert!(!linked.multi);

        // Not picked, a linked project has nothing to make a folder of.
        let err = create_linked(&linked, "x", &[], "other").unwrap_err();
        assert!(err.contains("no repository"), "{err}");
        assert!(!lib.join(".features").exists());

        // Its single repository, on the branch.
        let created = create_linked(&linked, "x", &["lib".into()], "other").unwrap();
        let dir = lib.join(".features/x");
        assert_eq!(created.dir, dir);
        assert_eq!(created.repos[0].project.as_deref(), Some("other"));
        assert!(created.repos[0].touched);
        assert!(branch_exists(&lib, "feature/x"));
        let exclude = std::fs::read_to_string(lib.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("/.features/"));
        f.links.push(link_of("other", &linked, &created));
        f.repos.extend(created.repos);
        assert_eq!(f.repos[1].key(), "other:lib");

        std::fs::write(dir.join("new"), "x").unwrap();
        let keys = ["other:lib".to_owned()];
        assert_eq!(losses(&f), ["other:lib: 1 changed file"]);
        assert_eq!(
            removal_losses(&f, &keys).unwrap(),
            ["other:lib: 1 changed file"]
        );
        let results = remove_repos(&f, &keys, false);
        let err = results[0].1.as_ref().unwrap_err();
        assert!(err.contains("would lose work"), "{err}");
        assert!(dir.is_dir());
        let err = remove(&f, false).unwrap_err();
        assert!(err.contains("other:lib"), "{err}");
        std::fs::remove_file(dir.join("new")).unwrap();

        // Taken out: its folder goes, the home one stays.
        assert_eq!(remove_repos(&f, &keys, false), [(keys[0].clone(), Ok(()))]);
        assert!(!lib.join(".features").exists());
        assert!(!branch_exists(&lib, "feature/x"));
        assert!(f.dir.is_dir());

        // Linked again and removed with the feature: both folders go.
        let created = create_linked(&linked, "x", &["lib".into()], "other").unwrap();
        assert!(created.repos[0].touched);
        f.repos.truncate(1);
        f.repos.extend(created.repos);
        remove(&f, false).unwrap();
        assert!(!lib.join(".features").exists());
        assert!(!home.join(".features").exists());
        assert!(!branch_exists(&lib, "feature/x"));
        assert!(!branch_exists(&home, "feature/x"));
    }

    #[test]
    fn a_feature_over_two_roots_is_made_whole_or_not_at_all() {
        let t = Temp::new();
        let home = t.repo("app", "main");
        let mut libs = Vec::new();
        for name in ["one", "two"] {
            let lib = t.0.join(name).join("lib");
            std::fs::create_dir_all(&lib).unwrap();
            run(&lib, &["init", "-q", "-b", "main"]);
            commit(&lib, "a", "first");
            libs.push(lib);
        }
        let root = find_root(&home, &[], &[], &[]).unwrap();
        let spec = |i: usize, touched: &[&str]| LinkSpec {
            project: format!("p{i}"),
            name: format!("Project {i}"),
            root: find_root(&libs[i], &[], &[], &[]).unwrap(),
            touched: touched.iter().map(|s| (*s).to_owned()).collect(),
        };
        // The second link can't be made: neither folder stays.
        std::fs::create_dir_all(libs[1].join(".features/x")).unwrap();
        let err =
            create_spanning(&root, "x", &[], &[spec(0, &["lib"]), spec(1, &["lib"])]).unwrap_err();
        assert!(err.starts_with("Project 1: "), "{err}");
        assert!(!home.join(".features/x").exists());
        assert!(!libs[0].join(".features").exists());
        assert!(!branch_exists(&libs[0], "feature/x"));
        assert!(!branch_exists(&home, "feature/x"));
        std::fs::remove_dir_all(libs[1].join(".features")).unwrap();

        let (made, links) =
            create_spanning(&root, "x", &[], &[spec(0, &["lib"]), spec(1, &["lib"])]).unwrap();
        assert_eq!(made.dir, home.join(".features/x"));
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].0.project, "p0");
        assert_eq!(links[0].0.dir, libs[0].join(".features/x"));
        assert!(links[0].1.repos[0].touched && links[1].1.repos[0].touched);
        assert_eq!(links[1].1.repos[0].key(), "p1:lib");
        let mut f = feature_of(&root, made, "x");
        for (link, created) in links {
            f.links.push(link);
            f.repos.extend(created.repos);
        }
        remove(&f, false).unwrap();
        for dir in [&home, &libs[0], &libs[1]] {
            assert!(!dir.join(".features").exists(), "{}", dir.display());
        }
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
    fn branches_of_lists_local_and_origins_once() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        run(&repo, &["branch", "next"]);
        for name in ["main", "dev"] {
            run(
                &repo,
                &["update-ref", &format!("refs/remotes/origin/{name}"), "HEAD"],
            );
        }
        run(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        run(
            &repo,
            &["update-ref", "refs/remotes/other/elsewhere", "HEAD"],
        );
        let mut all = branches_of(&repo).unwrap();
        all.sort();
        assert_eq!(all, ["dev", "main", "next"]);
    }

    #[test]
    fn following_origin_sees_past_a_stale_origin_head() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let found = |base: &str| Found {
            name: "app".into(),
            main: repo.clone(),
            base: base.into(),
        };
        let follow = |base: &str| {
            let mut found = found(base);
            follow_origin(&mut found);
            found.base
        };
        // No origin: nothing to ask.
        assert_eq!(follow("main"), "main");
        let bare = t.0.join("o.git");
        run(&t.0, &["init", "-q", "--bare", "-b", "dev", "o.git"]);
        run(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
        run(&repo, &["push", "-q", "origin", "main", "main:dev"]);
        // Never fetched: a worktree couldn't start from it.
        run(&repo, &["update-ref", "-d", "refs/remotes/origin/dev"]);
        assert_eq!(follow("main"), "main");
        run(&repo, &["fetch", "-q", "origin"]);
        run(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        assert_eq!(
            find_root(&repo, &[], &[], &[]).unwrap().repos[0].base,
            "main"
        );
        assert_eq!(follow("main"), "dev");
        // Unreachable: what git has here stands.
        run(&repo, &["remote", "set-url", "origin", "/nowhere/at/all"]);
        assert_eq!(follow("main"), "main");
    }

    #[test]
    fn a_base_only_on_origin_starts_a_worktree() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        run(&repo, &["update-ref", "refs/remotes/origin/dev", "HEAD"]);
        let mut root = find_root(&repo, &[], &[], &[]).unwrap();
        root.repos[0].base = "dev".into();
        let created = create(&root, "x", &[]).unwrap();
        assert_eq!(created.repos[0].base, "dev");
    }

    #[test]
    fn no_repository_is_an_error() {
        let t = Temp::new();
        let err = find_root(&t.0, &[], &[], &[]).unwrap_err();
        assert!(err.contains("is not in a git repository"), "{err}");
    }

    #[test]
    fn a_feature_folder_finds_the_root_it_sits_under() {
        // Multi: the feature folder holds only linked worktrees.
        let t = Temp::new();
        t.repo("api", "main");
        t.repo("web", "main");
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &["api".into()]).unwrap();
        let again = find_root(&created.dir, std::slice::from_ref(&created.dir), &[], &[]).unwrap();
        assert_eq!(again, root);
        let again = find_root(&created.dir.join("api"), &[], &[], &[]).unwrap();
        assert_eq!(again, root);

        // Single: the folder sits inside the repository.
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &[]).unwrap();
        assert_eq!(find_root(&created.dir, &[], &[], &[]).unwrap(), root);
    }

    #[test]
    fn a_worktree_in_a_feature_folder_names_its_main_worktree() {
        let t = Temp::new();
        let api = t.repo("api", "main");
        t.repo("web", "main");
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &["api".into()]).unwrap();
        assert_eq!(main_top(&created.dir.join("api")), Some(api));
        assert_eq!(main_top(&created.dir), None);
    }

    #[test]
    fn every_repository_of_the_project_is_offered_from_any_workspace() {
        let t = Temp::new();
        let app = t.0.join("app");
        let elsewhere = t.0.join("elsewhere");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let repo_in = |folder: &Path, name: &str| {
            let repo = folder.join(name);
            std::fs::create_dir(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            commit(&repo, "a", "first");
            repo
        };
        let api = repo_in(&app, "api");
        let web = repo_in(&app, "web");
        repo_in(&app, "unopened");
        let tool = repo_in(&elsewhere, "tool");
        // A plain folder's repositories aren't the project's when others are.
        let plain = t.0.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        repo_in(&plain, "stray");

        let dirs = [plain.clone(), api.clone(), web.clone(), tool.clone()];
        let all = ["api", "tool", "unopened", "web"];
        for from in [&api, &web, &tool, &plain] {
            let root = find_root(from, &dirs, &[], &[]).unwrap();
            assert!(root.multi);
            assert_eq!(root.path, app, "from {}", from.display());
            assert_eq!(names(&root), all);
            assert_eq!(root.repos[1].main, tool);
        }

        // A feature gets a worktree of the one from elsewhere too.
        let root = find_root(&api, &dirs, &[], &[]).unwrap();
        let created = create(&root, "x", &["tool".into()]).unwrap();
        assert_eq!(created.repos[0].worktree, app.join(".features/x/tool"));
        assert!(created.repos[0].touched);

        // Ignored ones stay out, wherever they are.
        let root = find_root(&api, &dirs, &["tool".into()], &[]).unwrap();
        assert_eq!(names(&root), ["api", "unopened", "web"]);
    }

    #[test]
    fn a_repository_of_another_project_stays_out() {
        let t = Temp::new();
        let app = t.0.join("app");
        let personal = t.0.join("personal");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&personal).unwrap();
        let repo_in = |folder: &Path, name: &str| {
            let repo = folder.join(name);
            std::fs::create_dir(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            commit(&repo, "a", "first");
            repo
        };
        let api = repo_in(&app, "api");
        let web = repo_in(&app, "web");
        let theirs = repo_in(&app, "theirs");
        let shared = repo_in(&app, "shared");
        let tool = repo_in(&personal, "tool");
        let gerdr = repo_in(&personal, "gerdr");
        // A plain folder among the workspaces, as a home folder is, and a
        // feature's folder.
        let home = t.0.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let features = app.join(FEATURES_DIR).join("x");
        std::fs::create_dir_all(&features).unwrap();
        let dirs = [
            home.clone(),
            api.clone(),
            web.clone(),
            shared.clone(),
            gerdr.clone(),
            tool.clone(),
            features.clone(),
        ];
        let others = [gerdr.clone(), theirs.clone(), shared.clone(), home];
        let names_from = |from: &Path| {
            let root = find_project_root(from, &dirs, &others, &[], &[]).unwrap();
            assert_eq!(root.path, app);
            names(&root).join(" ")
        };

        // Out: the workspace at another project's repository, and the one
        // the folder holds that only another project has. In: the one
        // nobody else has, and the one shared in this root.
        let own = "api shared tool web";
        assert_eq!(names_from(&features), own);
        assert_eq!(names_from(&api), own);
        // Started from it, it stays.
        assert_eq!(names_from(&gerdr), "api gerdr shared tool web");

        // With no other project all stay, as `find_root` found them.
        let all = find_project_root(&api, &dirs, &[], &[], &[]).unwrap();
        assert_eq!(
            names(&all),
            ["api", "gerdr", "shared", "theirs", "tool", "web"]
        );

        // A project on a repository of its own keeps it, shared or not.
        let root =
            find_project_root(&gerdr, std::slice::from_ref(&gerdr), &dirs, &[], &[]).unwrap();
        assert_eq!(names(&root), ["gerdr"]);

        // A project all of whose repositories others have keeps them.
        let everyone = [api.clone(), web.clone(), theirs.clone(), shared.clone()];
        let root =
            find_project_root(&app, std::slice::from_ref(&app), &everyone, &[], &[]).unwrap();
        assert_eq!(names(&root), ["api", "shared", "theirs", "web"]);
    }

    #[test]
    fn a_workspace_on_the_folder_brings_its_repositories_to_one_in_it() {
        let t = Temp::new();
        let api = t.repo("api", "main");
        t.repo("web", "main");
        let root = find_root(&api, &[t.0.clone(), api.clone()], &[], &[]).unwrap();
        assert!(root.multi);
        assert_eq!(names(&root), ["api", "web"]);
    }

    #[test]
    fn repositories_with_no_folder_in_common_are_only_those() {
        let t = Temp::new();
        let (a, b) = (t.0.join("a"), t.0.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for (folder, name) in [(&a, "one"), (&a, "not-opened"), (&b, "two")] {
            let repo = folder.join(name);
            std::fs::create_dir(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            commit(&repo, "a", "first");
        }
        let (one, two) = (a.join("one"), b.join("two"));
        let root = find_root(&two, &[one, two.clone()], &[], &[]).unwrap();
        assert!(root.multi);
        assert_eq!(root.path, b, "the folder of the one it started from");
        assert_eq!(names(&root), ["one", "two"]);
    }

    #[test]
    fn a_name_taken_in_the_root_keeps_the_roots_repository() {
        let t = Temp::new();
        let (a, b) = (t.0.join("a"), t.0.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for (folder, name) in [(&a, "api"), (&a, "web"), (&b, "api")] {
            let repo = folder.join(name);
            std::fs::create_dir(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            commit(&repo, "a", "first");
        }
        let dirs = [a.join("api"), a.join("web"), b.join("api")];
        let root = find_root(&b.join("api"), &dirs, &[], &[]).unwrap();
        assert_eq!(names(&root), ["api", "web"]);
        assert_eq!(root.repos[0].main, a.join("api"));
    }

    #[test]
    fn a_repository_from_elsewhere_takes_a_name_over_a_root_file() {
        let t = Temp::new();
        let (app, b) = (t.0.join("app"), t.0.join("b"));
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for (folder, name) in [(&app, "api"), (&app, "web"), (&b, "tool")] {
            let repo = folder.join(name);
            std::fs::create_dir(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            commit(&repo, "a", "first");
        }
        std::fs::write(app.join("tool"), "x").unwrap();
        let dirs = [app.join("api"), app.join("web"), b.join("tool")];
        let root = find_root(&app.join("api"), &dirs, &[], &[]).unwrap();
        let created = create(&root, "x", &["tool".into()]).unwrap();
        assert!(created.repos[0].worktree.join(".git").is_file());
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
    fn multi_creates_worktrees_of_the_ticked_repositories_with_links() {
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
        assert_eq!(created.repos.len(), 1);
        let w = &created.repos[0];
        assert!(w.touched);
        assert_eq!(
            run(&w.worktree, &["branch", "--show-current"]).trim(),
            "feature/auth"
        );
        assert!(!branch_exists(&api, "feature/auth"));
        assert!(branch_exists(&web, "feature/auth"));
        // The one left out has no folder, no link and no worktree.
        assert!(dir.join("api").symlink_metadata().is_err());
        assert_eq!(
            run(&api, &["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            2,
            "the main one and api-perf only"
        );

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
        assert_eq!(created.ignores, vec![t.0.join(CURSOR_INDEX)]);
        assert!(!t.0.join(".graphifyignore").exists());
        // Cursor's agents may read and edit in it: indexed no, denied no.
        assert_eq!(
            std::fs::read_to_string(t.0.join(CURSOR_DENY)).unwrap(),
            "target"
        );
        assert_eq!(
            std::fs::read_to_string(t.0.join(CURSOR_INDEX)).unwrap(),
            ".features/\n"
        );
        assert_eq!(
            std::fs::read_to_string(t.0.join("pyrightconfig.json")).unwrap(),
            r#"{"venv": ".venv"}"#
        );

        // A second feature changes no ignore file again.
        let second = create(&root, "other", &[]).unwrap();
        assert!(second.ignores.is_empty());

        // A feature made before holds a detached one: touch puts it on the
        // branch.
        let mut f = feature_of(&root, created, "auth");
        f.insert_repos(vec![context_repo(&api, &dir.join("api"))]);
        assert_eq!(f.repos[0].name, "api");
        assert_eq!(
            run(&f.repos[0].worktree, &["branch", "--show-current"]).trim(),
            ""
        );
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
        f.repos[0] = touched;

        // Untracked in the now touched api: a loss.
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
    fn a_project_without_cursor_files_gets_none() {
        let t = Temp::new();
        assert_eq!(cursor_index_rule(&t.0).unwrap(), None);
        assert_eq!(free_for_cursor(&t.0), Ok(false));
        assert!(!t.0.join(CURSOR_INDEX).exists());
    }

    #[test]
    fn the_indexing_rule_joins_what_is_there_once() {
        let t = Temp::new();
        let index = t.0.join(CURSOR_INDEX);
        std::fs::write(&index, "dist").unwrap();
        assert_eq!(cursor_index_rule(&t.0).unwrap(), Some(index.clone()));
        assert_eq!(cursor_index_rule(&t.0).unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(&index).unwrap(),
            "dist\n.features/\n"
        );
    }

    #[test]
    fn a_project_cursor_was_denied_is_freed_once_and_keeps_its_other_rules() {
        let t = Temp::new();
        let (deny, index) = (t.0.join(CURSOR_DENY), t.0.join(CURSOR_INDEX));
        std::fs::write(&deny, "# why\n**/dist/**\n\n.features/\n.tools/**").unwrap();

        assert_eq!(free_for_cursor(&t.0), Ok(true));
        assert_eq!(
            std::fs::read_to_string(&deny).unwrap(),
            "# why\n**/dist/**\n\n.tools/**"
        );
        assert_eq!(std::fs::read_to_string(&index).unwrap(), ".features/\n");

        // Nothing left to do, and the user's own edits stay.
        std::fs::write(&index, "").unwrap();
        assert_eq!(free_for_cursor(&t.0), Ok(false));
        assert_eq!(std::fs::read_to_string(&index).unwrap(), "");
    }

    #[test]
    fn only_lines_over_all_of_the_features_folder_go() {
        for line in [
            ".features",
            ".features/",
            "/.features/",
            "**/.features/**",
            ".features/**",
            "  .features/  ",
        ] {
            assert_eq!(
                without_features(&format!("a\n{line}\nb\n")).as_deref(),
                Some("a\nb\n")
            );
        }
        for text in [
            "",
            "a\n",
            ".features/a/\n",
            "x/.features/\n",
            "# .features/\n",
            "!.features/\n",
        ] {
            assert_eq!(without_features(text), None, "{text}");
        }
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
    fn a_code_graph_is_no_work_to_lose_and_goes_with_the_folder() {
        let t = Temp::new();
        t.repo("api", "main");
        t.repo("web", "main");
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "login", &[]).unwrap();
        let dir = created.dir.clone();
        let f = feature_of(&root, created, "login");
        // On top of a multi-repository folder, which no repository sees.
        let graph = dir.join(super::super::graph::OUT_DIR);
        std::fs::create_dir_all(&graph).unwrap();
        std::fs::write(graph.join("graph.json"), "{}").unwrap();
        assert!(losses(&f).is_empty());
        remove(&f, false).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn a_single_repositorys_kept_out_code_graph_goes_with_its_worktree() {
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let root = find_root(&repo, std::slice::from_ref(&repo), &[], &[]).unwrap();
        let created = create(&root, "login", &[]).unwrap();
        let dir = created.dir.clone();
        let f = feature_of(&root, created, "login");
        exclude_line(&dir, "/graphify-out/").unwrap();
        let graph = dir.join(super::super::graph::OUT_DIR);
        std::fs::create_dir_all(&graph).unwrap();
        std::fs::write(graph.join("graph.json"), "{}").unwrap();
        assert!(losses(&f).is_empty());
        remove(&f, false).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn a_branch_clash_counts_only_for_the_repositories_picked() {
        let t = Temp::new();
        let api = t.repo("api", "main");
        t.repo("web", "main");
        run(&api, &["branch", "feature/x"]);
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let err = create(&root, "x", &["api".into()]).unwrap_err();
        assert_eq!(err, "api already has a branch feature/x");
        assert!(create(&root, "x", &["web".into()]).is_ok());
    }

    #[test]
    fn a_repository_taken_out_comes_back_on_a_new_branch() {
        let t = Temp::new();
        let (_api, web) = (t.repo("api", "main"), t.repo("web", "main"));
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &["api".into(), "web".into()]).unwrap();
        let mut f = feature_of(&root, created, "x");
        let keys = ["web".to_owned()];
        // Uncommitted work the user agreed to lose stays lost.
        let lost = f.repos[1].worktree.join("lost");
        std::fs::write(&lost, "x").unwrap();
        assert_eq!(remove_repos(&f, &keys, true), [(keys[0].clone(), Ok(()))]);
        f.repos.retain(|r| r.key() != "web");
        f.note_removed(None, "web".into(), 7);
        assert!(!branch_exists(&web, "feature/x"));

        let added = add_repos(&f, None, &root, &["web".into()]).unwrap();
        f.insert_repos(added);
        assert!(f.removed.is_empty(), "it isn't removed anymore");
        let back = f.repo("web").unwrap();
        assert!(back.touched);
        assert_eq!(
            run(&back.worktree, &["branch", "--show-current"]).trim(),
            "feature/x"
        );
        assert!(!lost.exists());
    }

    #[test]
    fn repositories_join_a_feature_folder_later() {
        let t = Temp::new();
        let (api, web, zed) = (
            t.repo("api", "main"),
            t.repo("web", "main"),
            t.repo("zed", "main"),
        );
        std::fs::write(t.0.join("tool"), "a root file").unwrap();
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &["web".into()]).unwrap();
        let mut f = feature_of(&root, created, "x");

        let added = add_repos(&f, None, &root, &["zed".into(), "api".into()]).unwrap();
        assert!(added.iter().all(|r| r.touched));
        f.insert_repos(added);
        let keys: Vec<String> = f.repos.iter().map(Repo::key).collect();
        assert_eq!(keys, ["api", "web", "zed"]);
        assert_eq!(f.repos[0].worktree, f.dir.join("api"));
        assert_eq!(
            run(&f.repos[0].worktree, &["branch", "--show-current"]).trim(),
            "feature/x"
        );
        assert!(hook::guarded(&api, "feature/x"));

        // There already, and names it doesn't have.
        let err = add_repos(&f, None, &root, &["web".into()]).unwrap_err();
        assert!(err.contains("already has web"), "{err}");
        let err = add_repos(&f, None, &root, &["nope".into()]).unwrap_err();
        assert!(err.contains("no repository nope"), "{err}");

        // Another project's folder can't take them, nor one of one repository.
        let err = add_repos(&f, Some("gone"), &root, &["api".into()]).unwrap_err();
        assert!(err.contains("doesn't span gone"), "{err}");
        let mut moved = root.clone();
        moved.path = t.0.join("elsewhere");
        let (mut one, _) = (f.clone(), &web);
        one.repos.truncate(1);
        let err = add_repos(&one, None, &moved, &["web".into()]).unwrap_err();
        assert!(err.contains("moved"), "{err}");
        one.multi = false;
        let err = add_repos(&one, None, &root, &["web".into()]).unwrap_err();
        assert!(err.contains("one repository"), "{err}");
        assert!(zed.is_dir());
    }

    #[test]
    fn a_failed_add_takes_down_only_what_it_made() {
        let t = Temp::new();
        let (api, web, zed) = (
            t.repo("api", "main"),
            t.repo("web", "main"),
            t.repo("zed", "main"),
        );
        let bases = [("zed".to_owned(), "nope".to_owned())];
        let root = find_root(&t.0, &[], &[], &bases).unwrap();
        let created = create(&root, "x", &["web".into()]).unwrap();
        let f = feature_of(&root, created, "x");
        let err = add_repos(&f, None, &root, &["api".into(), "zed".into()]).unwrap_err();
        assert!(err.contains("nope"), "{err}");
        assert!(!branch_exists(&api, "feature/x"));
        assert!(!f.dir.join("api").exists());
        assert!(f.dir.join("web").is_dir());
        assert!(branch_exists(&web, "feature/x"));
        assert!(!branch_exists(&zed, "feature/x"));

        // A branch left behind stops it before anything is made.
        run(&zed, &["branch", "feature/x"]);
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let err = add_repos(&f, None, &root, &["api".into(), "zed".into()]).unwrap_err();
        assert_eq!(err, "zed already has a branch feature/x");
        assert!(!branch_exists(&api, "feature/x"));
    }

    #[test]
    fn a_failed_link_takes_down_all_it_made() {
        let t = Temp::new();
        let home = t.repo("app", "main");
        let (one, two) = (t.0.join("one"), t.0.join("two"));
        std::fs::create_dir(&one).unwrap();
        std::fs::create_dir(&two).unwrap();
        for dir in [one.join("lib"), two.join("a"), two.join("b"), two.join("c")] {
            std::fs::create_dir(&dir).unwrap();
            run(&dir, &["init", "-q", "-b", "main"]);
            commit(&dir, "a", "first");
        }
        let root = find_root(&home, &[], &[], &[]).unwrap();
        let mut f = feature_of(&root, create(&root, "x", &[]).unwrap(), "x");
        let bases = [("c".to_owned(), "nope".to_owned())];
        let linked = find_root(&two, &[], &[], &bases).unwrap();
        let created = create_linked(&linked, "x", &["a".into()], "two").unwrap();
        f.links.push(link_of("two", &linked, &created));
        f.insert_repos(created.repos);
        let new = LinkSpec {
            project: "one".into(),
            name: "One".into(),
            root: find_root(&one.join("lib"), &[], &[], &[]).unwrap(),
            touched: vec!["lib".into()],
        };
        let add = |names: &[&str]| AddSpec {
            project: Some("two".into()),
            name: "Two".into(),
            root: linked.clone(),
            names: names.iter().map(|n| (*n).to_owned()).collect(),
        };
        // c's base isn't there: the new link and b's worktree go again.
        let err =
            link_more(&f, std::slice::from_ref(&new), &[add(&["b"]), add(&["c"])]).unwrap_err();
        assert!(err.starts_with("Two: "), "{err}");
        assert!(!one.join("lib/.features").exists());
        assert!(!branch_exists(&one.join("lib"), "feature/x"));
        assert!(!branch_exists(&two.join("b"), "feature/x"));
        assert!(!two.join(".features/x/b").exists());
        assert!(two.join(".features/x/a").is_dir(), "what it held stays");

        let (links, repos) = link_more(&f, &[new], &[add(&["b"])]).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(repos[0].key(), "two:b");
    }

    #[test]
    fn a_root_file_of_its_name_stops_a_repository_joining() {
        let t = Temp::new();
        let (app, b) = (t.0.join("app"), t.0.join("b"));
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for (folder, name) in [(&app, "api"), (&app, "web"), (&b, "tool")] {
            let repo = folder.join(name);
            std::fs::create_dir(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            commit(&repo, "a", "first");
        }
        std::fs::write(app.join("tool"), "x").unwrap();
        let dirs = [app.join("api"), app.join("web"), b.join("tool")];
        let root = find_root(&app.join("api"), &dirs, &[], &[]).unwrap();
        let created = create(&root, "x", &["api".into()]).unwrap();
        let f = feature_of(&root, created, "x");
        let err = add_repos(&f, None, &root, &["tool".into()]).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn one_repository_leaves_and_the_folder_stays() {
        let t = Temp::new();
        let (api, web) = (t.repo("api", "main"), t.repo("web", "main"));
        std::fs::write(t.0.join("AGENTS.md"), "x").unwrap();
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &["api".into(), "web".into()]).unwrap();
        let mut f = feature_of(&root, created, "x");
        let keys = ["web".to_owned()];

        // Uncommitted work is said, and left without force.
        std::fs::write(f.repos[1].worktree.join("n"), "x").unwrap();
        assert_eq!(removal_losses(&f, &keys).unwrap(), ["web: 1 changed file"]);
        let results = remove_repos(&f, &keys, false);
        assert!(results[0].1.as_ref().unwrap_err().contains("lose work"));
        assert!(f.repos[1].worktree.is_dir());
        // Commits refuse it, however it is asked.
        commit(&f.repos[1].worktree, "n", "work");
        assert_eq!(
            removal_losses(&f, &keys).unwrap_err(),
            "web has 1 commit not in main"
        );
        run(&f.repos[1].worktree, &["reset", "-q", "--hard", "main"]);

        // Force takes uncommitted work, and the rest is as it was.
        std::fs::write(f.repos[1].worktree.join("n"), "x").unwrap();
        assert_eq!(remove_repos(&f, &keys, true), [(keys[0].clone(), Ok(()))]);
        assert!(!f.dir.join("web").exists());
        assert!(!branch_exists(&web, "feature/x"));
        assert!(!hook::guarded(&web, "feature/x"));
        assert!(f.dir.join("api").is_dir());
        assert!(f.dir.join("AGENTS.md").is_file());
        assert!(branch_exists(&api, "feature/x"));
        f.repos.retain(|r| r.name != "web");
        remove(&f, false).unwrap();
        assert!(!t.0.join(".features").exists());
    }

    #[test]
    fn a_read_only_repository_of_an_old_feature_can_leave() {
        let t = Temp::new();
        let (api, web) = (t.repo("api", "main"), t.repo("web", "main"));
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &["web".into()]).unwrap();
        let mut f = feature_of(&root, created, "x");
        f.insert_repos(vec![context_repo(&api, &f.dir.join("api"))]);
        let keys = ["api".to_owned()];
        assert_eq!(removal_refusal(&f, &keys), None);
        assert_eq!(removal_losses(&f, &keys).unwrap(), Vec::<String>::new());
        assert_eq!(remove_repos(&f, &keys, false), [(keys[0].clone(), Ok(()))]);
        assert!(!f.dir.join("api").exists());
        assert!(branch_exists(&web, "feature/x"));
    }

    #[test]
    fn a_feature_keeps_a_repository_to_change_and_its_own_folder() {
        let t = Temp::new();
        let (api, web) = (t.repo("api", "main"), t.repo("web", "main"));
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &["web".into()]).unwrap();
        let mut f = feature_of(&root, created, "x");
        let web_key = ["web".to_owned()];
        assert_eq!(
            removal_refusal(&f, &web_key).as_deref(),
            Some("a feature needs a repository to change")
        );
        // A read-only one left doesn't count.
        f.insert_repos(vec![context_repo(&api, &f.dir.join("api"))]);
        assert!(removal_refusal(&f, &web_key).is_some());
        assert!(web.is_dir());

        // A single repository is the feature's folder, with links or not.
        let t = Temp::new();
        let repo = t.repo("app", "main");
        let root = find_root(&repo, &[], &[], &[]).unwrap();
        let f = feature_of(&root, create(&root, "x", &[]).unwrap(), "x");
        let err = removal_refusal(&f, &["app".to_owned()]).unwrap();
        assert!(err.contains("own folder"), "{err}");
    }

    #[test]
    fn a_link_goes_with_its_last_repository() {
        let t = Temp::new();
        let home = t.repo("app", "main");
        let other = t.0.join("other");
        std::fs::create_dir(&other).unwrap();
        for name in ["lib", "docs"] {
            let repo = other.join(name);
            std::fs::create_dir(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            commit(&repo, "a", "first");
        }
        let root = find_root(&home, &[], &[], &[]).unwrap();
        let mut f = feature_of(&root, create(&root, "x", &[]).unwrap(), "x");
        let linked = find_root(&other, &[], &[], &[]).unwrap();
        let created = create_linked(&linked, "x", &["lib".into()], "other").unwrap();
        let dir = created.dir.clone();
        f.links.push(link_of("other", &linked, &created));
        f.insert_repos(created.repos);
        let added = add_repos(&f, Some("other"), &linked, &["docs".into()]).unwrap();
        f.insert_repos(added);
        let keys: Vec<String> = f.repos.iter().map(Repo::key).collect();
        assert_eq!(keys, ["app", "other:docs", "other:lib"]);

        // One goes: the folder stays for the other.
        let one = ["other:docs".to_owned()];
        assert_eq!(remove_repos(&f, &one, false), [(one[0].clone(), Ok(()))]);
        assert!(dir.join("lib").is_dir());
        f.repos.retain(|r| r.key() != one[0]);

        // The last one goes: so does the folder.
        let last = ["other:lib".to_owned()];
        assert_eq!(remove_repos(&f, &last, false), [(last[0].clone(), Ok(()))]);
        assert!(!dir.exists());
        assert!(!other.join(".features").exists());
    }

    #[test]
    fn a_home_folder_can_hold_no_repository() {
        let t = Temp::new();
        let (api, web) = (t.repo("api", "main"), t.repo("web", "main"));
        std::fs::write(t.0.join("AGENTS.md"), "x").unwrap();
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "x", &[]).unwrap();
        assert!(created.repos.is_empty());
        assert!(created.dir.join("AGENTS.md").is_file());
        assert!(!branch_exists(&api, "feature/x") && !branch_exists(&web, "feature/x"));
    }

    #[test]
    fn remove_keeps_going_when_a_worktree_is_gone() {
        let t = Temp::new();
        t.repo("api", "main");
        let web = t.repo("web", "main");
        let root = find_root(&t.0, &[], &[], &[]).unwrap();
        let created = create(&root, "g", &["web".into()]).unwrap();
        let f = feature_of(&root, created, "g");
        std::fs::remove_dir_all(&f.repos[0].worktree).unwrap();
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
