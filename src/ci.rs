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
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
};

use serde::{Deserialize, Deserializer, de::DeserializeOwned};

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
    /// else a pull request's checks page, else the branch's runs on the
    /// Actions page.
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
    let (host, slug) = crate::git::web_url(root)
        .as_deref()
        .and_then(github_repository)
        .ok_or_else(|| "not a GitHub repository".to_owned())?;
    let repo = if host == "github.com" {
        slug.clone()
    } else {
        key(&host, &slug)
    };
    let runs: Vec<Run> = gh(
        root,
        &["run", "list", "--repo", &repo, "-L", "100"],
        RUN_FIELDS,
    )?;
    let own: Vec<Pull> = gh(
        root,
        &[
            "pr", "list", "--repo", &repo, "--author", "@me", "--state", "open",
        ],
        OWN_PULL_FIELDS,
    )?;
    let default_branch = default_branch(root, &repo)?;
    let mut others = Vec::new();
    for number in watched_of(watched, &host, &slug) {
        if own.iter().any(|pull| pull.number == number) {
            continue;
        }
        let number = number.to_string();
        // One gone or unreadable doesn't keep the others from showing.
        match gh(root, &["pr", "view", &number, "--repo", &repo], PULL_FIELDS) {
            Ok(pull) => others.push(pull),
            Err(err) => tracing::debug!("pull request {number} of {repo}: {err}"),
        }
    }
    Ok(repo_ci(&host, &slug, &default_branch, &runs, &own, &others))
}

/// `fetch` each of `roots` off the event loop, on one thread, and `send`
/// what came of each.
pub fn read_all(
    roots: Vec<PathBuf>,
    watched: Watched,
    send: impl FnOnce(Vec<(PathBuf, Result<RepoCi, String>)>) + Send + 'static,
) {
    std::thread::spawn(move || {
        let read = roots
            .into_iter()
            .map(|root| {
                let ci = fetch(&root, &watched);
                if let Err(err) = &ci {
                    tracing::debug!("CI of {}: {err}", root.display());
                }
                (root, ci)
            })
            .collect();
        send(read)
    });
}

/// The pull request a GitHub page at `url` is about, any of its pages
/// (conversation, commits, checks, files): its host, `owner/repo` and
/// number.
pub fn pull_request_of(url: &str) -> Option<(String, String, u64)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let path = rest.split(['?', '#']).next()?;
    let mut parts = path.split('/');
    let (host, owner, repo) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts.next()?;
    if host.is_empty() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number = number.parse().ok().filter(|&n| n > 0)?;
    Some((host.to_ascii_lowercase(), format!("{owner}/{repo}"), number))
}

/// What of `now` is news, given how each row was `before` (keyed by
/// repository `key` and subject): a row that ran and now passed or
/// failed. A summary and a body per notification, with the link it opens.
/// A row seen for the first time isn't news.
pub fn ci_news(before: &HashMap<(String, Subject), CiState>, now: &[&RepoCi]) -> Vec<CiNews> {
    let mut news = Vec::new();
    for repo in now {
        let repo_key = key(&repo.host, &repo.slug);
        for row in &repo.rows {
            if before.get(&(repo_key.clone(), row.subject.clone())) != Some(&CiState::Running) {
                continue;
            }
            let verb = match row.state {
                CiState::Passed => "passed",
                CiState::Failed => "failed",
                CiState::Running | CiState::Unknown => continue,
            };
            let mut summary = format!("CI {verb}: {}", subject_label(&row.subject));
            if let Some(title) = row
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                summary.push(' ');
                summary.push_str(title);
            }
            let mut body = repo.slug.clone();
            let failed: Vec<&str> = row
                .checks
                .iter()
                .filter(|check| check.state == CiState::Failed)
                .map(|check| check.name.as_str())
                .collect();
            if row.state == CiState::Failed && !failed.is_empty() {
                body.push_str(" · ");
                body.push_str(&listed(&failed));
            }
            news.push(CiNews {
                summary,
                body,
                link: row.link.clone(),
            });
        }
    }
    news
}

/// The first few of `names`, and how many more there are.
fn listed(names: &[&str]) -> String {
    const SHOWN: usize = 3;
    let mut text = names[..names.len().min(SHOWN)].join(", ");
    if names.len() > SHOWN {
        text.push_str(&format!(" +{} more", names.len() - SHOWN));
    }
    text
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

/// What `gh` is asked for: the fields of a workflow run, of one of the
/// user's pull requests and of a watched one.
const RUN_FIELDS: &str = "databaseId,name,workflowName,displayTitle,headBranch,headSha,event,status,conclusion,url,createdAt";
const OWN_PULL_FIELDS: &str = "number,title,headRefName,url,statusCheckRollup";
const PULL_FIELDS: &str = "number,title,state,headRefName,url,statusCheckRollup";

/// A workflow run, as `gh run list` gives it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Run {
    #[serde(deserialize_with = "or_default")]
    database_id: u64,
    #[serde(deserialize_with = "or_default")]
    name: String,
    #[serde(deserialize_with = "or_default")]
    workflow_name: String,
    #[serde(deserialize_with = "or_default")]
    head_branch: String,
    #[serde(deserialize_with = "or_default")]
    head_sha: String,
    /// What started it: `push`, `pull_request`, `schedule`...
    #[serde(deserialize_with = "or_default")]
    event: String,
    #[serde(deserialize_with = "or_default")]
    status: String,
    #[serde(deserialize_with = "or_default")]
    conclusion: String,
    #[serde(deserialize_with = "or_default")]
    url: String,
    /// An RFC 3339 time in UTC, which sorts as text.
    #[serde(deserialize_with = "or_default")]
    created_at: String,
}

/// A pull request, as `gh pr list` and `gh pr view` give it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Pull {
    #[serde(deserialize_with = "or_default")]
    number: u64,
    #[serde(deserialize_with = "or_default")]
    title: String,
    /// `OPEN`, `MERGED` or `CLOSED`; only asked of watched ones.
    #[serde(deserialize_with = "or_default")]
    state: String,
    #[serde(deserialize_with = "or_default")]
    head_ref_name: String,
    #[serde(deserialize_with = "or_default")]
    status_check_rollup: Vec<Context>,
}

/// A check of a pull request's rollup: a check run (of Actions or another
/// app), or a commit status set through the API.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Context {
    #[serde(rename = "__typename", deserialize_with = "or_default")]
    kind: String,
    // A check run's.
    #[serde(deserialize_with = "or_default")]
    name: String,
    #[serde(deserialize_with = "or_default")]
    workflow_name: String,
    #[serde(deserialize_with = "or_default")]
    status: String,
    #[serde(deserialize_with = "or_default")]
    conclusion: String,
    #[serde(deserialize_with = "or_default")]
    details_url: String,
    // A status's.
    #[serde(deserialize_with = "or_default")]
    context: String,
    #[serde(deserialize_with = "or_default")]
    state: String,
    #[serde(deserialize_with = "or_default")]
    target_url: String,
}

impl Context {
    /// The check, and what tells it from the others: its workflow's name
    /// and its own, which a check run again keeps.
    fn check(&self) -> ((String, String), Check) {
        let status =
            self.kind == "StatusContext" || (self.name.is_empty() && !self.context.is_empty());
        if status {
            let state = if self.state.eq_ignore_ascii_case("pending") {
                CiState::Running
            } else {
                // SUCCESS, FAILURE and ERROR read as a conclusion would;
                // EXPECTED, a required status not set yet, is unknown.
                state_of("", &self.state)
            };
            let check = Check {
                name: self.context.clone(),
                state,
                link: some_text(&self.target_url),
            };
            return ((String::new(), self.context.clone()), check);
        }
        let check = Check {
            name: self.name.clone(),
            state: state_of(&self.status, &self.conclusion),
            link: some_text(&self.details_url),
        };
        ((self.workflow_name.clone(), self.name.clone()), check)
    }
}

/// What `gh repo view --json defaultBranchRef` gives.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RepoView {
    #[serde(deserialize_with = "or_default")]
    default_branch_ref: BranchRef,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BranchRef {
    #[serde(deserialize_with = "or_default")]
    name: String,
}

/// A field's value, its default when null.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn some_text(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// The host and `owner/repo` of a repository's web page `web`, when on
/// GitHub (or a GitHub Enterprise host).
fn github_repository(web: &str) -> Option<(String, String)> {
    let (host, path) = web.strip_prefix("https://")?.split_once('/')?;
    let host = host.to_ascii_lowercase();
    let (owner, repo) = path.split_once('/')?;
    if !host.contains("github") || owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((host, path.to_owned()))
}

/// The pull requests of `host` and `slug` watched. The key is matched
/// whatever its case, as GitHub's names are.
fn watched_of(watched: &Watched, host: &str, slug: &str) -> BTreeSet<u64> {
    let repo_key = key(host, slug);
    watched
        .iter()
        .filter(|(watched_key, _)| watched_key.eq_ignore_ascii_case(&repo_key))
        .flat_map(|(_, numbers)| numbers.iter().copied())
        .collect()
}

/// The default branches already asked of `gh`, by `--repo`: they hardly
/// change.
static DEFAULT_BRANCHES: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// The default branch of `repo`, asked of `gh` the first time.
fn default_branch(root: &Path, repo: &str) -> Result<String, String> {
    if let Some(name) = DEFAULT_BRANCHES
        .lock()
        .ok()
        .and_then(|known| known.get(repo).cloned())
    {
        return Ok(name);
    }
    let view: RepoView = gh(root, &["repo", "view", repo], "defaultBranchRef")?;
    let name = view.default_branch_ref.name;
    if let Ok(mut known) = DEFAULT_BRANCHES.lock() {
        known.insert(repo.to_owned(), name.clone());
    }
    Ok(name)
}

/// What `gh args`, run in `root`, prints of `fields` as JSON, or what it
/// says went wrong.
fn gh<T: DeserializeOwned>(root: &Path, args: &[&str], fields: &str) -> Result<T, String> {
    let what = args.get(..2).unwrap_or(args).join(" ");
    let out = Command::new("gh")
        .args(args)
        .args(["--json", fields])
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => "gh is not installed".to_owned(),
            _ => format!("gh: {err}"),
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.lines().map(str::trim).find(|l| !l.is_empty());
        return Err(why.map_or_else(|| format!("gh {what} failed"), str::to_owned));
    }
    serde_json::from_slice(&out.stdout).map_err(|err| format!("gh {what}: {err}"))
}

/// The CI of the repository `slug` on `host` out of what `gh` gave: its
/// recent `runs`, the user's `own` open pull requests and the `watched`
/// others.
fn repo_ci(
    host: &str,
    slug: &str,
    default_branch: &str,
    runs: &[Run],
    own: &[Pull],
    watched: &[Pull],
) -> RepoCi {
    let web = format!("https://{host}/{slug}");
    let mut rows: Vec<CiRow> = branches(default_branch)
        .into_iter()
        .filter_map(|name| branch_row(name, runs, &web))
        .collect();
    let mut pulls: Vec<CiRow> = own.iter().map(|pull| pull_row(pull, &web, false)).collect();
    let mut closed = Vec::new();
    for pull in watched {
        if own.iter().any(|o| o.number == pull.number) {
            continue;
        }
        if pull.state.eq_ignore_ascii_case("merged") || pull.state.eq_ignore_ascii_case("closed") {
            closed.push(pull.number);
        } else {
            pulls.push(pull_row(pull, &web, true));
        }
    }
    pulls.sort_by(|a, b| a.subject.cmp(&b.subject));
    rows.extend(pulls);
    let shown: BTreeSet<&str> = rows
        .iter()
        .filter_map(|row| match &row.subject {
            Subject::Branch(name) => Some(name.as_str()),
            Subject::Pr(_) => row.head.as_deref(),
        })
        .collect();
    let others_running = runs
        .iter()
        .filter(|run| running(&run.status) && !shown.contains(run.head_branch.as_str()))
        .map(|run| run.database_id)
        .collect::<BTreeSet<_>>()
        .len();
    RepoCi {
        slug: slug.to_owned(),
        host: host.to_owned(),
        rows,
        others_running,
        closed,
    }
}

/// The branches a card shows when they ran: the long-lived ones, the
/// default branch among them where it falls, else first.
fn branches(default_branch: &str) -> Vec<&str> {
    let mut names = LONG_LIVED.to_vec();
    if !default_branch.is_empty() && !names.contains(&default_branch) {
        names.insert(0, default_branch);
    }
    names
}

/// The row of branch `name`: the runs of its newest commit among `runs`,
/// the newest of each workflow; `None` when none ran. Runs of pull
/// requests from it are left out, as they show on the pull request. Its
/// link is its first failed run's, else the branch's runs on the Actions
/// page of `web`.
fn branch_row(name: &str, runs: &[Run], web: &str) -> Option<CiRow> {
    let mut ran: Vec<&Run> = runs
        .iter()
        .filter(|run| run.head_branch == name)
        .filter(|run| !matches!(run.event.as_str(), "pull_request" | "pull_request_target"))
        .collect();
    // Newest first.
    ran.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let newest = *ran.first()?;
    let mut checks: Vec<Check> = Vec::new();
    for run in ran.iter().filter(|run| run.head_sha == newest.head_sha) {
        let name = if run.workflow_name.is_empty() {
            &run.name
        } else {
            &run.workflow_name
        };
        if checks.iter().any(|check| check.name == *name) {
            continue;
        }
        checks.push(Check {
            name: name.clone(),
            state: state_of(&run.status, &run.conclusion),
            link: some_text(&run.url),
        });
    }
    checks.sort_by(|a, b| a.name.cmp(&b.name));
    Some(CiRow {
        subject: Subject::Branch(name.to_owned()),
        title: None,
        head: None,
        state: row_state(&checks),
        link: row_link(&checks, || format!("{web}/actions?query=branch%3A{name}")),
        checks,
        watched: false,
    })
}

/// The row of `pull`, whose repository's page is `web`: its checks'
/// rollup, a check run again in place of the one before it.
fn pull_row(pull: &Pull, web: &str, watched: bool) -> CiRow {
    let mut checks: Vec<((String, String), Check)> = Vec::new();
    for context in &pull.status_check_rollup {
        let (which, check) = context.check();
        match checks.iter_mut().find(|(seen, _)| *seen == which) {
            Some((_, seen)) => *seen = check,
            None => checks.push((which, check)),
        }
    }
    let checks: Vec<Check> = checks.into_iter().map(|(_, check)| check).collect();
    CiRow {
        subject: Subject::Pr(pull.number),
        title: Some(pull.title.clone()),
        head: Some(pull.head_ref_name.clone()),
        state: row_state(&checks),
        link: row_link(&checks, || format!("{web}/pull/{}/checks", pull.number)),
        checks,
        watched,
    }
}

/// Whether a run or check run with `status` is yet to finish.
fn running(status: &str) -> bool {
    !status.is_empty() && !status.eq_ignore_ascii_case("completed")
}

/// Where a run or check run stands, by its `status` then its
/// `conclusion`, as `gh` gives them (in any case).
fn state_of(status: &str, conclusion: &str) -> CiState {
    if running(status) {
        return CiState::Running;
    }
    match conclusion.to_ascii_lowercase().as_str() {
        "success" | "neutral" | "skipped" => CiState::Passed,
        "failure" | "timed_out" | "cancelled" | "startup_failure" | "action_required" | "error" => {
            CiState::Failed
        }
        _ => CiState::Unknown,
    }
}

/// Where a row with `checks` stands (see `CiRow::state`).
fn row_state(checks: &[Check]) -> CiState {
    let any = |state| checks.iter().any(|check| check.state == state);
    if any(CiState::Failed) {
        CiState::Failed
    } else if any(CiState::Running) {
        CiState::Running
    } else if any(CiState::Passed) {
        CiState::Passed
    } else {
        CiState::Unknown
    }
}

/// The page a row with `checks` opens: the first failed check's, else
/// `otherwise`.
fn row_link(checks: &[Check], otherwise: impl FnOnce() -> String) -> String {
    checks
        .iter()
        .filter(|check| check.state == CiState::Failed)
        .find_map(|check| check.link.clone())
        .unwrap_or_else(otherwise)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run as `gh run list` gives it.
    fn run(
        id: u64,
        workflow: &str,
        branch: &str,
        sha: &str,
        event: &str,
        end: &str,
        at: &str,
    ) -> String {
        let (status, conclusion) = match end {
            "in_progress" | "queued" => (end, ""),
            _ => ("completed", end),
        };
        format!(
            r#"{{"conclusion":"{conclusion}","createdAt":"{at}","databaseId":{id},"displayTitle":"Some commit","event":"{event}","headBranch":"{branch}","headSha":"{sha}","name":"{workflow}","status":"{status}","url":"https://github.com/o/r/actions/runs/{id}","workflowName":"{workflow}"}}"#
        )
    }

    fn runs(runs: &[String]) -> Vec<Run> {
        serde_json::from_str(&format!("[{}]", runs.join(","))).unwrap()
    }

    fn row<'a>(ci: &'a RepoCi, subject: &Subject) -> &'a CiRow {
        ci.rows.iter().find(|row| row.subject == *subject).unwrap()
    }

    fn branch(name: &str) -> Subject {
        Subject::Branch(name.into())
    }

    #[test]
    fn checks_stand_by_their_status_then_conclusion() {
        assert_eq!(state_of("in_progress", ""), CiState::Running);
        assert_eq!(state_of("QUEUED", ""), CiState::Running);
        assert_eq!(state_of("waiting", "failure"), CiState::Running);
        assert_eq!(state_of("completed", "success"), CiState::Passed);
        assert_eq!(state_of("COMPLETED", "NEUTRAL"), CiState::Passed);
        assert_eq!(state_of("completed", "skipped"), CiState::Passed);
        for failed in [
            "failure",
            "TIMED_OUT",
            "cancelled",
            "startup_failure",
            "action_required",
        ] {
            assert_eq!(state_of("completed", failed), CiState::Failed, "{failed}");
        }
        assert_eq!(state_of("completed", "stale"), CiState::Unknown);
        assert_eq!(state_of("completed", ""), CiState::Unknown);

        let check = |state: CiState| Check {
            name: String::new(),
            state,
            link: None,
        };
        use CiState::*;
        assert_eq!(
            row_state(&[check(Passed), check(Running), check(Failed)]),
            Failed
        );
        assert_eq!(row_state(&[check(Passed), check(Running)]), Running);
        assert_eq!(row_state(&[check(Unknown), check(Passed)]), Passed);
        assert_eq!(row_state(&[check(Unknown)]), Unknown);
        assert_eq!(row_state(&[]), Unknown);
    }

    #[test]
    fn a_branch_takes_the_runs_of_its_newest_commit() {
        let runs = runs(&[
            run(
                9,
                "Tests",
                "main",
                "new",
                "push",
                "in_progress",
                "2026-10-20T10:05:00Z",
            ),
            run(
                8,
                "Lint",
                "main",
                "new",
                "push",
                "failure",
                "2026-10-20T10:04:00Z",
            ),
            // A pull request's run from main is the pull request's.
            run(
                7,
                "Tests",
                "main",
                "pr",
                "pull_request",
                "success",
                "2026-10-20T10:06:00Z",
            ),
            // The commit before failed: no longer the branch's state.
            run(
                6,
                "Tests",
                "main",
                "old",
                "push",
                "failure",
                "2026-10-20T09:00:00Z",
            ),
            run(
                5,
                "Docs",
                "main",
                "old",
                "push",
                "failure",
                "2026-10-20T09:00:00Z",
            ),
            run(
                4,
                "Tests",
                "dev",
                "dev1",
                "push",
                "success",
                "2026-10-19T09:00:00Z",
            ),
            run(
                3,
                "Tests",
                "dev",
                "dev1",
                "workflow_dispatch",
                "failure",
                "2026-10-19T08:00:00Z",
            ),
        ]);
        let ci = repo_ci("github.com", "o/r", "main", &runs, &[], &[]);
        assert_eq!(
            ci.rows
                .iter()
                .map(|row| row.subject.clone())
                .collect::<Vec<_>>(),
            [branch("main"), branch("dev")]
        );
        let main = row(&ci, &branch("main"));
        assert_eq!(
            main.checks,
            [
                Check {
                    name: "Lint".into(),
                    state: CiState::Failed,
                    link: Some("https://github.com/o/r/actions/runs/8".into()),
                },
                Check {
                    name: "Tests".into(),
                    state: CiState::Running,
                    link: Some("https://github.com/o/r/actions/runs/9".into()),
                },
            ]
        );
        assert_eq!(main.state, CiState::Failed);
        assert_eq!(main.link, "https://github.com/o/r/actions/runs/8");
        assert!(!main.watched);

        // The newest run of a workflow on a commit stands for it.
        let dev = row(&ci, &branch("dev"));
        assert_eq!(dev.state, CiState::Passed);
        assert_eq!(dev.checks.len(), 1);
        assert_eq!(
            dev.link,
            "https://github.com/o/r/actions?query=branch%3Adev"
        );
    }

    #[test]
    fn the_default_branch_shows_where_it_falls_or_first() {
        assert_eq!(branches("dev"), ["main", "master", "dev", "develop"]);
        assert_eq!(
            branches("trunk"),
            ["trunk", "main", "master", "dev", "develop"]
        );
        assert_eq!(branches(""), ["main", "master", "dev", "develop"]);

        let runs = runs(&[
            run(
                3,
                "Tests",
                "develop",
                "a",
                "push",
                "success",
                "2026-10-20T10:00:00Z",
            ),
            run(
                2,
                "Tests",
                "trunk",
                "b",
                "push",
                "success",
                "2026-10-20T10:00:00Z",
            ),
            run(
                1,
                "Tests",
                "feature",
                "c",
                "push",
                "success",
                "2026-10-20T10:00:00Z",
            ),
        ]);
        let ci = repo_ci("github.com", "o/r", "trunk", &runs, &[], &[]);
        assert_eq!(
            ci.rows
                .iter()
                .map(|row| row.subject.clone())
                .collect::<Vec<_>>(),
            [branch("trunk"), branch("develop")]
        );
    }

    const ROLLUP: &str = r#"[{
        "headRefName": "fix-tabs",
        "number": 42,
        "title": "Fix tabs",
        "url": "https://github.com/o/r/pull/42",
        "statusCheckRollup": [
            {"__typename":"CheckRun","completedAt":"2026-10-20T16:44:21Z","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1/job/1","name":"build","startedAt":"2026-10-20T16:41:14Z","status":"COMPLETED","workflowName":"Tests"},
            {"__typename":"CheckRun","completedAt":"2026-10-20T16:43:37Z","conclusion":"SUCCESS","detailsUrl":"https://github.com/o/r/actions/runs/2/job/2","name":"lint","startedAt":"2026-10-20T16:40:49Z","status":"COMPLETED","workflowName":"Lint"},
            {"__typename":"CheckRun","completedAt":"2026-10-20T16:40:46Z","conclusion":"SKIPPED","detailsUrl":"https://github.com/o/r/actions/runs/3/job/3","name":"label","startedAt":"2026-10-20T16:40:46Z","status":"COMPLETED","workflowName":"Triage"},
            {"__typename":"StatusContext","context":"ci/circleci","state":"SUCCESS","targetUrl":"https://circleci.com/gh/o/r/1"},
            {"__typename":"CheckRun","completedAt":"0001-01-01T00:00:00Z","conclusion":"","detailsUrl":"https://github.com/o/r/actions/runs/1/job/4","name":"build","startedAt":"2026-10-20T17:00:00Z","status":"IN_PROGRESS","workflowName":"Tests"},
            {"__typename":"CheckRun","completedAt":"2026-10-20T16:41:56Z","conclusion":"SUCCESS","detailsUrl":"https://github.com/o/r/runs/5","name":"CodeQL","startedAt":"2026-10-20T16:41:53Z","status":"COMPLETED","workflowName":""}
        ]
    }, {
        "headRefName": "red",
        "number": 7,
        "title": "Red",
        "url": "https://github.com/o/r/pull/7",
        "statusCheckRollup": [
            {"__typename":"StatusContext","context":"deploy","state":"ERROR","targetUrl":""},
            {"__typename":"StatusContext","context":"ci/circleci","state":"FAILURE","targetUrl":"https://circleci.com/gh/o/r/2"},
            {"__typename":"StatusContext","context":"review","state":"PENDING","targetUrl":null}
        ]
    }, {
        "headRefName": "quiet",
        "number": 9,
        "title": "Quiet",
        "statusCheckRollup": null
    }]"#;

    #[test]
    fn a_pull_request_stands_by_its_checks_rollup() {
        let own: Vec<Pull> = serde_json::from_str(ROLLUP).unwrap();
        let ci = repo_ci("github.com", "o/r", "main", &[], &own, &[]);
        // Pull requests by number.
        assert_eq!(
            ci.rows
                .iter()
                .map(|row| row.subject.clone())
                .collect::<Vec<_>>(),
            [Subject::Pr(7), Subject::Pr(9), Subject::Pr(42)]
        );

        // The build run again runs, in the place of the failed one.
        let tabs = row(&ci, &Subject::Pr(42));
        assert_eq!(
            tabs.checks
                .iter()
                .map(|c| (c.name.as_str(), c.state))
                .collect::<Vec<_>>(),
            [
                ("build", CiState::Running),
                ("lint", CiState::Passed),
                ("label", CiState::Passed),
                ("ci/circleci", CiState::Passed),
                ("CodeQL", CiState::Passed),
            ]
        );
        assert_eq!(tabs.state, CiState::Running);
        assert_eq!(tabs.link, "https://github.com/o/r/pull/42/checks");
        assert_eq!(tabs.title.as_deref(), Some("Fix tabs"));
        assert_eq!(tabs.head.as_deref(), Some("fix-tabs"));
        assert!(!tabs.watched);

        // A failed status without a page: the next failed one's opens.
        let red = row(&ci, &Subject::Pr(7));
        assert_eq!(
            red.checks.iter().map(|c| c.state).collect::<Vec<_>>(),
            [CiState::Failed, CiState::Failed, CiState::Running]
        );
        assert_eq!(red.state, CiState::Failed);
        assert_eq!(red.link, "https://circleci.com/gh/o/r/2");

        let quiet = row(&ci, &Subject::Pr(9));
        assert_eq!(quiet.state, CiState::Unknown);
        assert!(quiet.checks.is_empty());
    }

    #[test]
    fn runs_about_no_row_are_counted() {
        let runs = runs(&[
            run(
                1,
                "Tests",
                "main",
                "a",
                "push",
                "in_progress",
                "2026-10-20T10:00:00Z",
            ),
            run(
                2,
                "Tests",
                "fix-tabs",
                "b",
                "pull_request",
                "queued",
                "2026-10-20T10:00:00Z",
            ),
            run(
                3,
                "Tests",
                "feature",
                "c",
                "push",
                "in_progress",
                "2026-10-20T10:00:00Z",
            ),
            run(
                4,
                "Lint",
                "feature",
                "c",
                "push",
                "queued",
                "2026-10-20T10:00:00Z",
            ),
            run(
                4,
                "Lint",
                "feature",
                "c",
                "push",
                "queued",
                "2026-10-20T10:00:00Z",
            ),
            run(
                5,
                "Tests",
                "other",
                "d",
                "push",
                "success",
                "2026-10-20T10:00:00Z",
            ),
        ]);
        let own: Vec<Pull> = serde_json::from_str(ROLLUP).unwrap();
        let ci = repo_ci("github.com", "o/r", "main", &runs, &own, &[]);
        assert_eq!(ci.others_running, 2);
        assert!(ci.busy());
    }

    #[test]
    fn a_watched_pull_request_found_closed_is_dropped() {
        let watched: Vec<Pull> = serde_json::from_str(
            r#"[
                {"number":3,"title":"Theirs","state":"OPEN","headRefName":"theirs","url":"https://github.com/o/r/pull/3","statusCheckRollup":[]},
                {"number":4,"title":"Merged","state":"MERGED","headRefName":"merged","url":"https://github.com/o/r/pull/4","statusCheckRollup":[]},
                {"number":5,"title":"Closed","state":"CLOSED","headRefName":"closed","url":"https://github.com/o/r/pull/5","statusCheckRollup":[]},
                {"number":42,"title":"Fix tabs","state":"OPEN","headRefName":"fix-tabs","statusCheckRollup":[]}
            ]"#,
        )
        .unwrap();
        let own: Vec<Pull> = serde_json::from_str(ROLLUP).unwrap();
        let ci = repo_ci("github.com", "o/r", "main", &[], &own, &watched);
        assert_eq!(ci.closed, [4, 5]);
        assert_eq!(
            ci.rows
                .iter()
                .map(|row| (row.subject.clone(), row.watched))
                .collect::<Vec<_>>(),
            [
                (Subject::Pr(3), true),
                (Subject::Pr(7), false),
                (Subject::Pr(9), false),
                (Subject::Pr(42), false),
            ]
        );
    }

    #[test]
    fn watched_pull_requests_are_found_by_their_repository() {
        let watched: Watched = [
            ("github.com/Owner/Repo".to_owned(), BTreeSet::from([1, 2])),
            ("github.com/owner/other".to_owned(), BTreeSet::from([3])),
        ]
        .into();
        assert_eq!(
            watched_of(&watched, "github.com", "owner/repo"),
            BTreeSet::from([1, 2])
        );
        assert!(watched_of(&watched, "ghe.example.com", "owner/repo").is_empty());
    }

    #[test]
    fn github_repositories_are_told_from_others() {
        assert_eq!(
            github_repository("https://GitHub.com/owner/repo"),
            Some(("github.com".into(), "owner/repo".into()))
        );
        assert_eq!(
            github_repository("https://github.example.com/owner/repo"),
            Some(("github.example.com".into(), "owner/repo".into()))
        );
        assert_eq!(github_repository("https://gitlab.com/owner/repo"), None);
        assert_eq!(github_repository("https://github.com/group/sub/repo"), None);
        assert_eq!(github_repository("https://github.com/owner"), None);
    }

    #[test]
    fn pull_request_pages_are_recognized() {
        let pull = |host: &str, slug: &str, n| Some((host.to_owned(), slug.to_owned(), n));
        assert_eq!(
            pull_request_of("https://github.com/o/r/pull/42"),
            pull("github.com", "o/r", 42)
        );
        assert_eq!(
            pull_request_of("https://GitHub.com/o/r/pull/42/checks?check_run_id=1#step:3:9"),
            pull("github.com", "o/r", 42)
        );
        assert_eq!(
            pull_request_of("http://ghe.example.com/o/r/pull/7/files"),
            pull("ghe.example.com", "o/r", 7)
        );
        assert_eq!(
            pull_request_of("https://github.com/o/r/pull/42#issuecomment-1"),
            pull("github.com", "o/r", 42)
        );
        for not in [
            "https://github.com/o/r/pulls/42",
            "https://github.com/o/r/pull/",
            "https://github.com/o/r/pull/abc",
            "https://github.com/o/r/pull/+4",
            "https://github.com/o/r/pull/0",
            "https://github.com/o/r/issues/42",
            "https://github.com/o/r",
            "https://github.com//r/pull/42",
            "ftp://github.com/o/r/pull/42",
            "github.com/o/r/pull/42",
            "",
        ] {
            assert_eq!(pull_request_of(not), None, "{not}");
        }
    }

    fn repo(rows: Vec<CiRow>) -> RepoCi {
        RepoCi {
            slug: "o/r".into(),
            host: "github.com".into(),
            rows,
            others_running: 0,
            closed: Vec::new(),
        }
    }

    fn ci_row(subject: Subject, title: Option<&str>, state: CiState, failed: &[&str]) -> CiRow {
        CiRow {
            subject,
            title: title.map(str::to_owned),
            head: None,
            state,
            checks: failed
                .iter()
                .map(|name| Check {
                    name: (*name).into(),
                    state: CiState::Failed,
                    link: None,
                })
                .collect(),
            link: "https://example.com/link".into(),
            watched: false,
        }
    }

    #[test]
    fn a_row_that_ran_and_finished_is_news() {
        let was = |subject: Subject, state| (("github.com/o/r".to_owned(), subject), state);
        let before: HashMap<_, _> = [
            was(branch("main"), CiState::Running),
            was(branch("dev"), CiState::Running),
            was(Subject::Pr(42), CiState::Running),
            was(Subject::Pr(43), CiState::Passed),
            was(Subject::Pr(44), CiState::Running),
        ]
        .into();
        let now = repo(vec![
            ci_row(branch("main"), None, CiState::Passed, &[]),
            ci_row(branch("dev"), None, CiState::Running, &[]),
            ci_row(
                Subject::Pr(42),
                Some(" Fix tabs "),
                CiState::Failed,
                &["a", "b", "c", "d", "e"],
            ),
            ci_row(
                Subject::Pr(43),
                Some("Done before"),
                CiState::Failed,
                &["a"],
            ),
            ci_row(Subject::Pr(44), Some("Unknown now"), CiState::Unknown, &[]),
            // First seen.
            ci_row(Subject::Pr(45), Some("New"), CiState::Failed, &["a"]),
        ]);
        assert_eq!(
            ci_news(&before, &[&now]),
            [
                CiNews {
                    summary: "CI passed: main".into(),
                    body: "o/r".into(),
                    link: "https://example.com/link".into(),
                },
                CiNews {
                    summary: "CI failed: #42 Fix tabs".into(),
                    body: "o/r · a, b, c +2 more".into(),
                    link: "https://example.com/link".into(),
                },
            ]
        );
        assert_eq!(listed(&["a", "b"]), "a, b");
    }
}
