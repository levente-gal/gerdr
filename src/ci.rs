//! The CI of the shown project's GitHub repositories, read through the
//! `gh` CLI off the event loop, for the CI card under each workspace of a
//! repository (see `chrome::CiView`).
//!
//! A repository's card shows, unasked, its long-lived branches (`main`,
//! `master`, `dev`, `develop` and its default branch) and the user's own
//! open pull requests; any other pull request only once watched, from the
//! tab strip's button while a GitHub tab shows it. The runs of anything
//! else are only counted, for the card to point at the Actions page.
//!
//! A branch's state is that of the GitHub Actions runs of its newest
//! commit (`gh run list`); a pull request's is its checks' rollup
//! (`gh pr list` / `gh pr view`), which counts checks from outside Actions
//! too.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    time::Duration,
};

/// How often the CI is read while some of it runs, and while all of it is
/// settled.
pub const BUSY_INTERVAL: Duration = Duration::from_secs(20);
pub const IDLE_INTERVAL: Duration = Duration::from_secs(90);

/// The branches a card shows without being asked, besides the default
/// branch.
pub const LONG_LIVED: &[&str] = &["main", "master", "dev", "develop"];

/// Where a run, a check or a whole row stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CiState {
    /// Queued, waiting or in progress.
    Running,
    /// Finished, and all of it passed (or was skipped, or neutral).
    Passed,
    /// Finished, or still running, with something failed, timed out or
    /// cancelled.
    Failed,
    /// Nothing ran (yet), or nothing is known.
    #[default]
    Unknown,
}

/// What a row of the card is about.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subject {
    Branch(String),
    Pr(u64),
}

/// One check of a row: a workflow run of a branch, a check run or status
/// of a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: CiState,
    /// Its page, if GitHub gives one.
    pub link: Option<String>,
}

/// A row of the card: a branch or a pull request, and its checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRow {
    pub subject: Subject,
    /// A pull request's title.
    pub title: Option<String>,
    /// The branch a pull request comes from.
    pub head: Option<String>,
    /// All its checks together: `Failed` if any failed, else `Running` if
    /// any runs, else `Passed`; `Unknown` without checks.
    pub state: CiState,
    pub checks: Vec<Check>,
    /// The page a click opens: the first failed check's when one failed,
    /// else a pull request's checks page, else the branch's newest run.
    pub link: String,
    /// Shown because the user watches it (not one of theirs, nor a
    /// long-lived branch).
    pub watched: bool,
}

/// A repository's CI as last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoCi {
    /// `owner/repo`, as `--repo` takes it with `host/` before it off
    /// github.com.
    pub slug: String,
    pub host: String,
    /// Long-lived branches first (in `LONG_LIVED` order, the default
    /// branch where it falls), then pull requests by number. Branches
    /// without a run among the recent ones are left out.
    pub rows: Vec<CiRow>,
    /// How many runs not about a row are queued or in progress.
    pub others_running: usize,
    /// Watched pull requests found merged or closed: no longer watched.
    pub closed: Vec<u64>,
}

impl RepoCi {
    /// The repository's web page, `https://<host>/<slug>`.
    pub fn web_url(&self) -> String {
        format!("https://{}/{}", self.host, self.slug)
    }

    /// Its Actions page.
    pub fn actions_url(&self) -> String {
        format!("{}/actions", self.web_url())
    }

    /// Whether some row, or another run, is still running: the CI is then
    /// read every `BUSY_INTERVAL`.
    pub fn busy(&self) -> bool {
        self.others_running > 0 || self.rows.iter().any(|r| r.state == CiState::Running)
    }
}

/// The pull requests watched, by `host/slug` (see `key`).
pub type Watched = BTreeMap<String, BTreeSet<u64>>;

/// The key of a repository in `Watched`: `host/owner/repo`.
pub fn key(host: &str, slug: &str) -> String {
    format!("{host}/{slug}")
}

/// Read the CI of the repository at `root`, its pull requests in
/// `watched` included: blocking, it runs `git` and `gh`. An error says why
/// it can't be read (not a GitHub repository, `gh` missing or signed out).
pub fn fetch(root: &Path, watched: &Watched) -> Result<RepoCi, String> {
    let _ = (root, watched);
    todo!()
}

/// `fetch` each of `roots` off the event loop, on one thread, and `send`
/// what came of each.
pub fn read_all(
    roots: Vec<PathBuf>,
    watched: Watched,
    send: impl FnOnce(Vec<(PathBuf, Result<RepoCi, String>)>) + Send + 'static,
) {
    let _ = (roots, watched, send);
    todo!()
}

/// The pull request a GitHub page at `url` is about, any of its pages
/// (conversation, commits, checks, files): its host, `owner/repo` and
/// number.
pub fn pull_request_of(url: &str) -> Option<(String, String, u64)> {
    let _ = url;
    todo!()
}

/// What of `now` is news, given how each row was `before` (keyed by
/// repository `key` and subject): a row that ran and now passed or
/// failed. A summary and a body per notification, with the link it opens.
/// A row seen for the first time isn't news.
pub fn ci_news(before: &HashMap<(String, Subject), CiState>, now: &[&RepoCi]) -> Vec<CiNews> {
    let _ = (before, now);
    todo!()
}

/// A notification about a row that finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiNews {
    pub summary: String,
    pub body: String,
    pub link: String,
}

/// How `subject` shows on the card: a branch's name, `#N` for a pull
/// request.
pub fn subject_label(subject: &Subject) -> String {
    match subject {
        Subject::Branch(name) => name.clone(),
        Subject::Pr(n) => format!("#{n}"),
    }
}
