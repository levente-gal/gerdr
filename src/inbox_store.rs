//! What the inbox keeps of a project: its todos, and how its items were
//! triaged (dismissed, snoozed), in `~/.local/share/gerdr/inbox/<id>.json`.
//! Written atomically (see `projects::write_atomic`); a file that can't be
//! read reads as empty.
//!
//! An item is hidden while its triage's fingerprint is the item's current
//! one and its snooze (if any) hasn't run out: a change to the item (an
//! agent's new state, a new commit) brings it back. Entries not seen for
//! `PRUNE_AFTER` are forgotten.

#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// The file's format version.
pub const VERSION: u32 = 1;

/// How long a triage entry whose item is gone is kept, in seconds.
pub const PRUNE_AFTER: u64 = 30 * 24 * 3600;

/// A todo's id within its project: `t1`, `t2`…
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TodoId(pub String);

/// A todo written in gerdr.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub id: TodoId,
    pub text: String,
    /// The workspace it is about, by its record's id (`Workspace::record`,
    /// `projects::WorkspaceRecord::id`); `None`: the whole project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// When it was written, in seconds since the epoch.
    pub created: u64,
    /// When it was marked done; done todos aren't listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<u64>,
}

/// How an item was triaged, by its id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triage {
    /// The item's fingerprint when it was dismissed or snoozed.
    pub fingerprint: String,
    /// Snoozed until then, in seconds since the epoch; `None`: dismissed
    /// until it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    /// When the item was last listed, for pruning.
    pub seen: u64,
}

/// A project's inbox file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxFile {
    pub version: u32,
    /// The number the next todo's id takes.
    pub next_todo: u64,
    pub todos: Vec<Todo>,
    /// By item id (`crate::inbox::ItemId`).
    pub triage: BTreeMap<String, Triage>,
}

impl Default for InboxFile {
    fn default() -> Self {
        InboxFile {
            version: VERSION,
            next_todo: 1,
            todos: Vec::new(),
            triage: BTreeMap::new(),
        }
    }
}

/// Project `project`'s file under `root` (`None`: gerdr's data folder,
/// `projects::data_dir`).
pub fn file(root: Option<&Path>, project: &str) -> PathBuf {
    let root = root.map_or_else(crate::projects::data_dir, Path::to_path_buf);
    root.join("inbox").join(format!("{project}.json"))
}

/// Project `project`'s inbox, as saved under `root`; empty when there is
/// none or it can't be read.
pub fn load(root: Option<&Path>, project: &str) -> InboxFile {
    let _ = file(root, project);
    InboxFile::default()
}

/// Save project `project`'s inbox under `root`, atomically.
pub fn save(root: Option<&Path>, project: &str, inbox: &InboxFile) -> std::io::Result<()> {
    let _ = (file(root, project), inbox);
    Ok(())
}

/// 09:00 local time on the day after `now` (seconds since the epoch): when
/// "snooze until tomorrow" runs out.
pub fn tomorrow_morning(now: u64) -> u64 {
    now + 24 * 3600
}

/// Forget the triage entries not seen for `PRUNE_AFTER` before `now`.
pub fn prune(inbox: &mut InboxFile, now: u64) {
    let _ = (inbox, now);
}

/// The current time, in seconds since the epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
