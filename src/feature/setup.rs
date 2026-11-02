//! Setting up a feature's worktrees after they are made: the commands the
//! project's store file gives per repository (`"workflow": {"setup":
//! {"services-api": ["npm ci"]}}`), run in that repository's worktree.
//!
//! Before a repository's commands, each folder of `REFLINK` its main
//! worktree has and its feature worktree lacks is copied over with
//! `cp -a --reflink=always`: on a copy-on-write file system that is
//! instant and takes no space, and `npm ci` then has little to do. Where
//! the file system can't, the copy fails, its leftovers go, and the
//! commands run as they would.
//!
//! Every command runs with `sh -c`, stdin closed, its output appended to
//! `setup.log` in the feature data dir, for at most `TIMEOUT`. The first
//! one that fails stops the setup: its repository, command and exit go in
//! the error.
//!
//! Runs processes: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::Feature;

/// The folders copied from a repository's main worktree before its setup.
pub const REFLINK: &[&str] = &["node_modules"];

/// How long one setup command may run.
pub const TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// The setup log's name, in the feature data dir.
pub const LOG: &str = "setup.log";

/// Where a feature's setup stands, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupRun {
    /// Unix seconds.
    pub started: u64,
    /// When it ended, either way (unix seconds); `None` while it runs, or
    /// when gerdr quit before it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<u64>,
    /// Why it failed: "services-api: `npm ci` exited with 1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// Whether `commands` (the store file's setup, by repository name) give
/// `feature` anything to run, `REFLINK` copies aside.
pub fn has_commands(feature: &Feature, commands: &BTreeMap<String, Vec<String>>) -> bool {
    let _ = (feature, commands);
    false
}

/// Set up every repository of `feature` with a worktree: the `REFLINK`
/// copies, then its `commands`, logging to `log`. `progress` is told
/// what runs, in words: "services-api: npm ci (2 of 3)".
pub fn run(
    feature: &Feature,
    commands: &BTreeMap<String, Vec<String>>,
    log: &Path,
    progress: &mut dyn FnMut(String),
) -> Result<(), String> {
    let _ = (feature, commands, log, progress);
    Ok(())
}

/// The log of the feature with data dir `data`.
pub fn log_path(data: &Path) -> PathBuf {
    data.join(LOG)
}
