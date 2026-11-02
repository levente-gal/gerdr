//! Verify: gerdr runs the project's checks on a feature's code and writes
//! what they said to `evidence.md` in the feature data dir.
//!
//! The checks come from the project's store file, by repository name or
//! `_feature` (`"workflow": {"verify": {"services-api": ["npm test"],
//! "_feature": ["./e2e.sh"]}}`): each touched repository's run in its
//! worktree, in name order, then `_feature`'s in the feature folder. An
//! untouched repository's checks don't run.
//!
//! Verify fails when:
//! - the code reads dirty, or an untouched repository has changes
//!   (`patch::Reading::dirty`, `stray`): nothing runs;
//! - a check exits non-zero or runs past the timeout; every check still
//!   runs, so the evidence is whole;
//! - the code reads differently after the checks than before: a check
//!   changed it (a formatter, a snapshot update).
//!
//! Every check runs with `sh -c`, stdin closed, for at most the timeout;
//! the evidence keeps the last `TAIL` lines of its output.
//!
//! Runs processes: call it off the UI thread (checklist D3).

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use super::Feature;

/// The checks' key for the feature folder as a whole.
pub const FEATURE_KEY: &str = "_feature";

/// The evidence file's name, in the feature data dir.
pub const EVIDENCE: &str = "evidence.md";

/// How long one check may run, unless the store file says otherwise
/// (`"workflow": {"verify_timeout": 900}`, seconds).
pub const TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// How many of a check's last output lines the evidence keeps.
pub const TAIL: usize = 40;

/// One check to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// The repository's name, or `FEATURE_KEY`.
    pub repo: String,
    /// Where it runs: the repository's worktree, or the feature folder.
    pub dir: PathBuf,
    pub command: String,
}

/// What one check did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub repo: String,
    pub command: String,
    /// Its exit code; `None` when it was killed (timed out) or couldn't
    /// start.
    pub exit: Option<i32>,
    pub timed_out: bool,
    /// Why it couldn't start, if it couldn't.
    pub error: Option<String>,
    /// How long it ran, in seconds.
    pub secs: u64,
    /// Its output's last lines, stdout and stderr as they came.
    pub tail: String,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.exit == Some(0)
    }
}

/// What Verify found: what `evidence.md` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    /// Unix seconds it started.
    pub at: u64,
    /// The feature's patch-id the checks ran on; empty when it couldn't
    /// be read.
    pub patch: String,
    /// Each touched repository's `HEAD` commit, by name.
    pub heads: Vec<(String, String)>,
    pub outcomes: Vec<Outcome>,
    /// What failed outside the checks, one line each: "uncommitted
    /// changes in services-api", "services-web isn't touched but has
    /// changes", "the checks changed the code in services-api",
    /// "couldn't read the code: …".
    pub problems: Vec<String>,
}

impl Evidence {
    /// Whether every check passed and nothing else failed.
    pub fn passed(&self) -> bool {
        self.problems.is_empty() && self.outcomes.iter().all(Outcome::passed)
    }

    /// What failed, one short line each, problems first:
    /// "services-api: `npm test` exited with 1".
    pub fn failures(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The checks `config` (by repository name or `FEATURE_KEY`) gives
/// `feature`, in the order they run.
pub fn checks(feature: &Feature, config: &BTreeMap<String, Vec<String>>) -> Vec<Check> {
    let _ = (feature, config);
    Vec::new()
}

/// Run `checks` on `feature`'s code, each for at most `timeout`;
/// `progress` is told what runs: "services-api: npm test (2 of 5)".
pub fn run(
    feature: &Feature,
    checks: &[Check],
    timeout: Duration,
    progress: &mut dyn FnMut(String),
) -> Evidence {
    let _ = (feature, checks, timeout, progress);
    Evidence::default()
}

/// `evidence.md`'s text for `evidence` of `feature`.
pub fn markdown(feature: &Feature, evidence: &Evidence) -> String {
    let _ = (feature, evidence);
    String::new()
}

/// A feature's latest Verify run, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyRun {
    /// Which run: 1, 2…
    pub n: u32,
    /// Unix seconds.
    pub started: u64,
    /// When it ended (unix seconds); `None` while it runs, or when gerdr
    /// quit before it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<u64>,
    /// The patch-id it ran on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Whether it passed, once finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    /// What failed (`Evidence::failures`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
    /// How many failed runs in a row were sent to the Build agent to fix:
    /// gerdr stops sending after `MAX_FIXES`, and the user decides.
    #[serde(default)]
    pub fixes: u32,
}

/// How many failed Verify runs in a row gerdr sends to the Build agent.
pub const MAX_FIXES: u32 = 3;
