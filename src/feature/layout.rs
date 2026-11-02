//! A feature's folder of worktrees beside the project's repositories, and
//! finding those repositories.
//!
//! Every function here runs git and touches the disk: call it off the UI
//! thread (checklist D3) and hand the result back over a channel.

use std::path::{Path, PathBuf};

use super::{Feature, Repo};

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
    let _ = (dir, workspace_dirs, ignore, bases);
    Err("not built yet".into())
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
    let _ = (root, slug, touched);
    Err("not built yet".into())
}

/// Put `repo`'s worktree of `feature` on branch `feature/<slug>` at the
/// commit it is on, so the feature can change it. Returns the repository
/// as it is now.
pub fn touch(feature: &Feature, repo: &str) -> Result<Repo, String> {
    let _ = (feature, repo);
    Err("not built yet".into())
}

/// What taking `feature`'s worktrees down would lose, one line per
/// repository that loses something ("services-api: 2 changed files, 3
/// commits not in dev"). Empty when nothing is lost.
pub fn losses(feature: &Feature) -> Vec<String> {
    let _ = feature;
    Vec::new()
}

/// Take `feature`'s worktrees and folder down, and delete its branches.
/// `force` takes them down even when that loses work (`losses`); without
/// it, a repository that would lose something is left and named in the
/// error. A branch with commits not in its base is kept unless `force`.
pub fn remove(feature: &Feature, force: bool) -> Result<(), String> {
    let _ = (feature, force);
    Err("not built yet".into())
}
