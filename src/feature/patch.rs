//! A feature's patch-id: one id for the change it makes over all its
//! repositories, which stays the same while the change does (a rebase onto
//! a moved base keeps it) and differs once the code does.
//!
//! Per touched repository, `git diff <merge-base> HEAD | git patch-id
//! --stable` gives the id of its committed change against its base; the
//! feature's patch-id is `git hash-object` of the lines `<repo> <id>`, in
//! repository name order. Only commits count: uncommitted changes are
//! reported apart (`Reading::dirty`), and a bound stage can't be marked
//! while there are any.
//!
//! Runs git: call it off the UI thread (checklist D3).

use std::collections::BTreeMap;

use super::Feature;

/// What reading a feature's worktrees found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// The feature's patch-id: 40 hex digits. The same for a feature
    /// without commits as for any other one without.
    pub patch: String,
    /// Touched repositories with uncommitted changes (untracked files
    /// count, ignored ones don't), by name.
    pub dirty: Vec<String>,
    /// Repositories the feature doesn't touch whose worktree has changes,
    /// by name: those wouldn't ship.
    pub stray: Vec<String>,
    /// Per touched repository, its commits not in its base.
    pub commits: BTreeMap<String, usize>,
}

/// Read `feature`'s worktrees: its patch-id, and what isn't committed. An
/// error says which repository couldn't be read and why.
pub fn read(feature: &Feature) -> Result<Reading, String> {
    let _ = feature;
    Err("not built yet".into())
}
