//! Code review's viewed ticks: the files of the feature's diff the user
//! marked viewed, each kept with the blob it had then. A file whose blob
//! changed since reads as not viewed, marked "changed"; the others keep
//! their tick across code changes.
//!
//! Keyed `<repo>/<path>`: the repository's folder name (`Repo::name`) and
//! the path from its top folder. The blob is the file's committed one on
//! the feature's branch (`git diff --raw <base>...HEAD`'s new blob); a
//! deleted file's is all zeros.
//!
//! Files with changes not committed: the diff shows them as they are on
//! disk, which no committed blob is. Such a file can't be ticked (commit
//! it first), and one ticked before reads as changed until its changes
//! are committed and it is viewed again, or they are undone and its
//! committed blob is still the one ticked. An untracked file has no
//! committed blob at all: it can't be ticked either.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    Repo,
    layout::{base_ref, git_bytes},
};
use crate::{diff::FileDiff, git_ops::Since};

/// The viewed files, in the feature record (`Feature::viewed`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Viewed(pub BTreeMap<String, String>);

/// A file's tick, as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    No,
    Yes,
    /// Viewed, but the file changed since.
    Changed,
}

/// The key of file `path` of repository `repo`.
pub fn key(repo: &str, path: &str) -> String {
    format!("{repo}/{path}")
}

/// Why a file can't be ticked: it has changes not committed.
pub const NOT_COMMITTED: &str = "Commit it first: viewed marks the committed file";

impl Viewed {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The tick of file `key`, whose committed blob is `blob` (none for
    /// one not committed), `dirty` when it has changes not committed.
    pub fn tick(&self, key: &str, blob: Option<&str>, dirty: bool) -> Tick {
        match self.0.get(key) {
            None => Tick::No,
            Some(kept) if !dirty && Some(kept.as_str()) == blob => Tick::Yes,
            Some(_) => Tick::Changed,
        }
    }

    /// Toggle file `key`'s tick: a ticked one (as `Yes`) is unticked;
    /// another ticked on `blob`. Refused, with why, for a file with
    /// changes not committed. Whether it is ticked now.
    pub fn toggle(
        &mut self,
        key: &str,
        blob: Option<&str>,
        dirty: bool,
    ) -> Result<bool, &'static str> {
        if self.tick(key, blob, dirty) == Tick::Yes {
            self.0.remove(key);
            return Ok(false);
        }
        match blob {
            Some(blob) if !dirty => {
                self.0.insert(key.to_owned(), blob.to_owned());
                Ok(true)
            }
            _ => Err(NOT_COMMITTED),
        }
    }
}

/// What code review reads of one touched repository, off the UI thread:
/// its changes since it forked from its base, their diff, each changed
/// file's committed blob and the files with changes not committed.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub since: Since,
    pub files: Vec<FileDiff>,
    /// By path: the new blob of `git diff --raw <fork> HEAD`.
    pub blobs: BTreeMap<String, String>,
    /// Paths with changes not committed, or untracked.
    pub dirty: BTreeSet<String>,
}

/// Read `repo`'s worktree against its base (see `Read`); slow, run it off
/// the UI thread.
pub fn read(repo: &Repo) -> Result<Read, String> {
    let dir = &repo.worktree;
    let target = base_ref(repo);
    let since = crate::git_ops::changes_since(dir, Some(&target))?;
    let files = crate::git_ops::repo_diff_from(dir, &since.changes, Some(&since.base));
    let args = [
        "diff",
        "--raw",
        "-z",
        "--no-abbrev",
        "-M",
        &since.base,
        "HEAD",
    ];
    let blobs = parse_raw(&git_bytes(dir, &args, None)?);
    let status = ["status", "--porcelain", "-z", "--untracked-files=all"];
    let dirty = parse_status(&git_bytes(dir, &status, None)?);
    Ok(Read {
        since,
        files,
        blobs,
        dirty,
    })
}

/// The new blob of each path in `git diff --raw -z --no-abbrev` output;
/// a rename's or copy's under its new path.
pub fn parse_raw(out: &[u8]) -> BTreeMap<String, String> {
    let text = String::from_utf8_lossy(out);
    let mut parts = text.split('\0');
    let mut blobs = BTreeMap::new();
    while let Some(meta) = parts.next() {
        let Some(meta) = meta.strip_prefix(':') else {
            continue;
        };
        let fields: Vec<&str> = meta.split(' ').collect();
        let (Some(blob), Some(status)) = (fields.get(3), fields.get(4)) else {
            continue;
        };
        // A rename or copy names the old path, then the new.
        if status.starts_with(['R', 'C']) {
            parts.next();
        }
        if let Some(path) = parts.next() {
            blobs.insert(path.to_owned(), (*blob).to_owned());
        }
    }
    blobs
}

/// The paths `git status --porcelain -z` lists: changed in the index or
/// on disk, or untracked; a rename's new path.
pub fn parse_status(out: &[u8]) -> BTreeSet<String> {
    let text = String::from_utf8_lossy(out);
    let mut parts = text.split('\0');
    let mut dirty = BTreeSet::new();
    while let Some(entry) = parts.next() {
        let (Some(code), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        dirty.insert(path.to_owned());
        // A rename's old path comes next.
        if code.contains(['R', 'C']) {
            parts.next();
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_holds_while_the_committed_blob_is_the_one_viewed() {
        let mut v = Viewed::default();
        let k = key("api", "src/a.rs");
        assert_eq!(k, "api/src/a.rs");
        assert_eq!(v.tick(&k, Some("b1"), false), Tick::No);
        assert_eq!(v.toggle(&k, Some("b1"), false), Ok(true));
        assert_eq!(v.tick(&k, Some("b1"), false), Tick::Yes);
        // Committed again: changed since viewed.
        assert_eq!(v.tick(&k, Some("b2"), false), Tick::Changed);
        // Changes not committed: changed too, and can't be ticked.
        assert_eq!(v.tick(&k, Some("b1"), true), Tick::Changed);
        assert_eq!(v.toggle(&k, Some("b1"), true), Err(NOT_COMMITTED));
        assert_eq!(v.toggle(&k, None, false), Err(NOT_COMMITTED));
        // A changed one ticks again on its new blob; a ticked one unticks.
        assert_eq!(v.toggle(&k, Some("b2"), false), Ok(true));
        assert_eq!(v.0.get(&k).map(String::as_str), Some("b2"));
        assert_eq!(v.toggle(&k, Some("b2"), false), Ok(false));
        assert!(v.is_empty());
    }

    #[test]
    fn raw_diff_gives_each_paths_new_blob() {
        let z = "0000000000000000000000000000000000000000";
        let out = format!(
            ":100644 100644 aaa bbb M\0src/a.rs\0\
             :000000 100644 {z} ccc A\0new.rs\0\
             :100644 100644 ddd eee R087\0old.rs\0moved.rs\0\
             :100644 000000 fff {z} D\0gone.rs\0"
        );
        let blobs = parse_raw(out.as_bytes());
        let get = |p: &str| blobs.get(p).map(String::as_str);
        assert_eq!(get("src/a.rs"), Some("bbb"));
        assert_eq!(get("new.rs"), Some("ccc"));
        assert_eq!(get("moved.rs"), Some("eee"));
        assert_eq!(get("old.rs"), None);
        assert_eq!(get("gone.rs"), Some(z));
    }

    #[test]
    fn status_lists_changed_and_untracked_paths() {
        let out = " M src/a.rs\0R  new.rs\0old.rs\0?? notes.md\0";
        let dirty = parse_status(out.as_bytes());
        let all: Vec<&str> = dirty.iter().map(String::as_str).collect();
        assert_eq!(all, ["new.rs", "notes.md", "src/a.rs"]);
    }
}
