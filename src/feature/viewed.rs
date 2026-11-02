//! Code review's viewed ticks: the files of the feature's diff the user
//! marked viewed, each kept with the blob it had then. A file whose blob
//! changed since reads as not viewed, marked "changed"; the others keep
//! their tick across code changes.
//!
//! Keyed `<repo>/<path>`: the repository's folder name (`Repo::name`) and
//! the path from its top folder. The blob is the file's committed one on
//! the feature's branch (`git diff --raw <base>...HEAD`'s new blob); a
//! deleted file's is all zeros.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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

impl Viewed {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
