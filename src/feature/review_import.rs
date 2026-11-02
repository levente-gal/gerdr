//! AI review's findings: what the reviewer agent wrote to `review.json`,
//! read in and kept in the feature record (`Feature::review`), and what
//! the user decided of each.
//!
//! `review.json` (the reviewer writes it in the feature data dir):
//!
//! ```json
//! {"findings": [{"id": "r1-2", "severity": "high", "repo": "api",
//!   "file": "src/a.rs", "line": 12, "title": "…", "body": "…", "fix": "…"}]}
//! ```
//!
//! - `severity`: `high`, `medium` or `low`.
//! - `repo`: the repository's folder name; may be left out for a feature
//!   over one repository.
//! - `file`: from the repository's top folder. `line`: on the new side of
//!   the diff, 1-based; may be left out for the whole file.
//! - `id`: only to raise a finding of an earlier run again (the prompt
//!   lists them); gerdr gives new ones their own, `r<run>-<k>`.
//!
//! The file is untrusted: it is capped (`MAX_BYTES`, `MAX_FINDINGS`, each
//! text `MAX_TEXT` characters), control characters and HTML tags are taken
//! out, and a finding on a file or line not in the diff is kept but not
//! `placed`.
//!
//! What the user decides (`Decision`): Fix, or Dismiss with a reason. Once
//! none is `Open`, the ones to fix go to the Build agent together (state
//! `Sent`); its fix changes the code, the feature goes back to Build, and
//! after Verify the reviewer runs again on what changed since it last
//! read the code (`AiReview::reviewed`), told which findings were sent to
//! be fixed. A sent finding it doesn't raise again is `Fixed`. AI review
//! holds once no finding is `Open`, `Fix` or `Sent`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The file the reviewer writes, in the feature data dir.
pub const REVIEW: &str = "review.json";
/// The largest `review.json` read.
pub const MAX_BYTES: u64 = 256 * 1024;
/// The most findings kept of one run.
pub const MAX_FINDINGS: usize = 100;
/// The most characters kept of a finding's title, body or fix.
pub const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    High,
    #[default]
    Medium,
    Low,
}

impl Severity {
    /// As the diff and panel show it: `high`.
    pub fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// Where a finding stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingState {
    /// Not decided yet.
    #[default]
    Open,
    /// The user wants it fixed; not sent yet.
    Fix,
    /// Sent to the Build agent to fix.
    Sent,
    /// Sent, and the next review didn't raise it again.
    Fixed,
    /// The user dismissed it (`Finding::reason` says why).
    Dismissed,
}

impl FindingState {
    /// Whether AI review still waits on it.
    pub fn holds_up(self) -> bool {
        matches!(self, Self::Open | Self::Fix | Self::Sent)
    }
}

/// One finding of the reviewer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// `r<run>-<k>`: unique within the feature.
    pub id: String,
    pub severity: Severity,
    /// The repository's folder name (`Repo::name`).
    pub repo: String,
    /// From the repository's top folder.
    pub file: String,
    /// On the new side of the diff, 1-based; none for the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// What the reviewer suggests doing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fix: String,
    #[serde(default)]
    pub state: FindingState,
    /// Why it was dismissed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The review run that raised it last.
    pub run: u32,
    /// Its file (and line) is in the diff it was read against.
    #[serde(default)]
    pub placed: bool,
}

/// What the user decides of a finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Fix,
    /// Dismissed, with the reason (not empty).
    Dismiss(String),
    /// Back to undecided (from Fix or Dismissed).
    Reopen,
}

/// The feature's AI review, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReview {
    /// The latest run read in: 1, 2…
    pub n: u32,
    /// Every finding of every run, in the order raised.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    /// Per touched repository, the commit its code was at when the last
    /// run read it: the next run reviews what changed since.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reviewed: BTreeMap<String, String>,
    /// The feature's patch-id the last run was read on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

impl AiReview {
    /// How many findings AI review still waits on.
    pub fn open(&self) -> usize {
        self.findings.iter().filter(|f| f.state.holds_up()).count()
    }

    /// The finding `id`.
    pub fn finding(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == id)
    }
}
