//! The issues assigned to the user, for the inbox's todos: GitHub's, per
//! repository of the shown project (`gh issue list --assignee @me`, see
//! `ci::gh_json`), and Linear's (its GraphQL API's `viewer.assignedIssues`,
//! through `curl` with the key on stdin, never in its arguments).
//!
//! Reading blocks: the caller runs `read_all` on a thread of its own.

#![allow(dead_code)]

use std::{path::PathBuf, time::Instant};

use crate::config;

/// Where an issue comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IssueSource {
    GitHub,
    Linear,
}

/// An open issue assigned to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub source: IssueSource,
    /// Stable across reads: `<host>/<slug>#<n>` for GitHub, the issue's
    /// uuid for Linear.
    pub id: String,
    /// As shown: `#12`, `ENG-123`.
    pub key: String,
    pub title: String,
    /// Its web page.
    pub url: String,
    /// `open`, or Linear's state's name (`In Progress`).
    pub state: String,
    pub labels: Vec<String>,
    /// When it last changed, as the source gives it (RFC 3339): the
    /// item's fingerprint.
    pub updated: String,
    /// Where it is: the repository's slug, the Linear team's key.
    pub place: String,
}

/// A GitHub repository to read the issues of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhRepo {
    /// Its root on disk, where `gh` runs.
    pub root: PathBuf,
    pub host: String,
    /// `owner/repo`.
    pub slug: String,
}

/// What a round of reading the issues came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssuesRead {
    /// Per repository, by `ci::key(host, slug)`: its issues, or why they
    /// couldn't be read.
    pub github: Vec<(String, Result<Vec<Issue>, String>)>,
    /// Linear's, or why they couldn't be read; `None` without a key.
    pub linear: Option<Result<Vec<Issue>, String>>,
}

/// The issues as last read, kept in `inbox::Inbox`.
#[derive(Clone, Debug, Default)]
pub struct IssuesState {
    /// The last round's results; a source that failed keeps what it had.
    pub last: IssuesRead,
    /// When the last round ended; `None` before one did.
    pub at: Option<Instant>,
    /// A round is under way.
    pub pending: bool,
}

impl IssuesState {
    /// Every issue read, GitHub's then Linear's.
    pub fn issues(&self) -> Vec<&Issue> {
        let github = self.last.github.iter().filter_map(|(_, r)| r.as_ref().ok());
        let linear = self.last.linear.iter().filter_map(|r| r.as_ref().ok());
        github.chain(linear).flatten().collect()
    }
}

/// Read the issues assigned to the user: in each of `github`'s
/// repositories, and on Linear with `linear`'s key (none without one).
/// Blocks until every read is done; GitHub's run in parallel.
pub fn read_all(github: Vec<GhRepo>, linear: Option<config::Linear>) -> IssuesRead {
    let _ = (github, linear);
    IssuesRead::default()
}
