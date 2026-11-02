//! The `pre-push` hook guarding features' branches.
//!
//! gerdr installs one `pre-push` hook per hooks folder a feature's worktree
//! uses (`git rev-parse --git-path hooks`, which follows `core.hooksPath`).
//! It chains to a hook that was there. For each ref pushed it looks for a
//! guard file, `<git common dir>/gerdr/guard/<branch>`: none, and the push
//! goes on (a branch that isn't a gerdr feature's isn't guarded); there,
//! and the push goes on only when the commit pushed is the one the file
//! names. gerdr writes that commit right before it pushes the branch when
//! shipping (`allow`) and empties the file after (`guard`), so any other
//! push of the branch is refused with a line saying to ship it from gerdr.
//!
//! Runs git and writes in the repository's git folder: call it off the UI
//! thread (checklist D3).

use std::path::Path;

/// Whether pushes from a worktree are guarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Guard {
    On,
    /// Not guarded, and why, in words: "its hooks folder .husky/_ isn't
    /// there yet".
    Off(String),
}

/// Make sure the hook is in the hooks folder `worktree` uses, chaining to
/// a hook that was there: installs it, updates an older one of gerdr's, or
/// leaves it. A hook folder whose `pre-push` git tracks is not touched.
pub fn ensure(worktree: &Path) -> Guard {
    let _ = worktree;
    Guard::Off("not built yet".into())
}

/// Guard `branch` of the repository `dir` is in: a push of it is refused
/// until `allow`.
pub fn guard(dir: &Path, branch: &str) -> Result<(), String> {
    let _ = (dir, branch);
    Err("not built yet".into())
}

/// Let `branch` be pushed at commit `commit` (a full hash), and only at it.
pub fn allow(dir: &Path, branch: &str, commit: &str) -> Result<(), String> {
    let _ = (dir, branch, commit);
    Err("not built yet".into())
}

/// Stop guarding `branch`: its feature is gone. Nothing to remove is fine.
pub fn unguard(dir: &Path, branch: &str) -> Result<(), String> {
    let _ = (dir, branch);
    Err("not built yet".into())
}
