//! A pull request on GitHub reviewed the way a feature is: a `Feature` of
//! `Kind::PrReview`, its worktree at the pull request's head
//! (`layout::create_pr`), in `<root>/.features/pr-<n>/`. It is born with
//! Discover, Plan, Plan review and Build marked by "pull request", as its
//! author did those, Build on the code of its head; its review goes
//! through the stages from Verify on (`STAGES`). Its brief is the pull
//! request's title and description (`brief`), so the stages that read one
//! (an agent's prompt, Manual test's criteria) read that.
//!
//! The pull request is the user's own when they wrote it on GitHub
//! (`PullRef::mine`); from a branch of this repository, gerdr then works on
//! that branch (`PullRef::own`). Someone else's, or one from a fork, gets a
//! branch of gerdr's that is never pushed.
//!
//! Reading one runs gh: call it off the UI thread (checklist D3).

use std::{collections::BTreeMap, path::Path, sync::Mutex};

use serde::{Deserialize, Serialize};

use crate::ci::CiState;

use super::{
    Event, Feature, Kind, Mark, Stage, VERSION,
    layout::{Created, Root},
};

/// The stages a pull request's review goes through, in order.
pub const STAGES: [Stage; 5] = [
    Stage::Verify,
    Stage::AiReview,
    Stage::CodeReview,
    Stage::ManualTest,
    Stage::Ship,
];

/// Who marked the stages its author did.
pub const BY: &str = "pull request";

/// What `gh pr view` is asked for.
const FIELDS: &str = "number,title,body,author,baseRefName,headRefName,headRefOid,\
                      isCrossRepository,url,state,isDraft";

/// A pull request a review is of, as it was last read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRef {
    /// The repository of the review it is of, by `Repo::key`; empty in a
    /// record of one pull request, from before reviews held several: its
    /// first repository's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo: String,
    /// `github.com`, or a GitHub Enterprise host.
    pub host: String,
    /// `owner/repo`.
    pub slug: String,
    pub number: u64,
    /// Its page.
    pub url: String,
    /// Its author's login.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// The user wrote it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mine: bool,
    /// The branch it goes into: `main`.
    pub base_ref: String,
    /// Its branch, in the repository it comes from.
    pub head_ref: String,
    /// The commit its head was at when the worktree was last moved to it.
    pub head_sha: String,
    /// It comes from a fork.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cross_repo: bool,
    /// How GitHub had it when last read (see `pr_watch`), kept from when
    /// that last changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<Remote>,
    /// Someone else's, fixed here: what its review wants goes to an agent
    /// here, and its fixes are pushed to its branch from Ship's page, as
    /// for the user's own (`works_here`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fix_here: bool,
    /// The repository its branch is in, `owner/repo`, when that isn't
    /// `slug` (a fork's), as read when fixing it here was asked for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub head_repo: String,
}

/// A pull request as GitHub had it when last read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// Unix seconds it was first read so.
    pub at: u64,
    pub head_sha: String,
    pub base_ref: String,
    /// `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    /// Its checks on `head_sha`, each once (`ci::rollup_checks`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CiCheck>,
}

/// One check GitHub ran on a pull request's head: a check run of GitHub
/// Actions or of an app, or a commit status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCheck {
    pub name: String,
    pub state: CiState,
    /// Its page, if GitHub gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// What reading a pull request again changed of what was kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Nothing,
    /// Only how its checks stand.
    Checks,
    /// Its head, base or state.
    Pull,
}

/// How a pull request's checks on the head reviewed stand, all told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// None ran, or none was reported.
    None,
    /// Some haven't finished.
    Running,
    /// All finished, some not passed.
    Failed,
    Passed,
}

/// The checks `checks` counted: how many there are, passed and failed (or
/// of an unknown end), the rest still running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ci {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub running: usize,
}

impl Ci {
    pub fn of(checks: &[CiCheck]) -> Self {
        let count =
            |want: &dyn Fn(CiState) -> bool| checks.iter().filter(|c| want(c.state)).count();
        Self {
            total: checks.len(),
            passed: count(&|s| s == CiState::Passed),
            failed: count(&|s| matches!(s, CiState::Failed | CiState::Unknown)),
            running: count(&|s| s == CiState::Running),
        }
    }

    pub fn verdict(&self) -> Verdict {
        match () {
            _ if self.total == 0 => Verdict::None,
            _ if self.running > 0 => Verdict::Running,
            _ if self.failed > 0 => Verdict::Failed,
            _ => Verdict::Passed,
        }
    }
}

/// The GitHub Actions runs of `checks` that didn't pass, by id, from their
/// pages (`…/actions/runs/<id>/job/…`): those `gh run rerun --failed` takes.
/// Other checks (an app's, a status) can't be run again from here.
pub fn rerun_ids(checks: &[CiCheck]) -> Vec<u64> {
    let mut ids: Vec<u64> = (checks.iter())
        .filter(|c| matches!(c.state, CiState::Failed | CiState::Unknown))
        .filter_map(|c| {
            let link = c.link.as_deref()?;
            let (_, after) = link.split_once("/actions/runs/")?;
            after.split(['/', '?', '#']).next()?.parse().ok()
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

impl Remote {
    /// Whether `other` says the same of it, whenever read, but for its
    /// checks.
    pub fn same(&self, other: &Remote) -> bool {
        self.head_sha == other.head_sha
            && self.base_ref == other.base_ref
            && self.state.eq_ignore_ascii_case(&other.state)
    }

    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }

    pub fn merged(&self) -> bool {
        self.state.eq_ignore_ascii_case("merged")
    }
}

impl PullRef {
    /// The user's own, from a branch of this repository: gerdr works on
    /// that branch, and its fixes can be pushed to it.
    pub fn own(&self) -> bool {
        self.mine && !self.cross_repo
    }

    /// Its review's fixes are made here and pushed to its branch: the
    /// user's own, or someone else's they chose to fix (`fix_here`).
    pub fn works_here(&self) -> bool {
        self.own() || self.fix_here
    }

    /// Where its fixes are pushed: `origin` for a branch of this repository,
    /// else its fork's address, made from origin's (`origin`).
    pub fn push_remote(&self, origin: &str) -> Option<String> {
        match self.cross_repo && !self.head_repo.is_empty() {
            false => Some("origin".to_owned()),
            true => fork_url(origin, &self.slug, &self.head_repo),
        }
    }

    /// The slug of its review: `pr-12`.
    pub fn feature_slug(&self) -> String {
        slug_of(self.number)
    }

    /// The branch its worktree is on: its own for the user's own, else
    /// `gerdr/pr-<n>`.
    pub fn branch(&self) -> String {
        match self.own() {
            true => self.head_ref.clone(),
            false => format!("gerdr/pr-{}", self.number),
        }
    }

    /// Where its head is fetched to: `refs/gerdr/pr/<n>`.
    pub fn fetch_ref(&self) -> String {
        format!("refs/gerdr/pr/{}", self.number)
    }

    /// What has its head as GitHub has it: origin's branch for the user's
    /// own, else `fetch_ref`. Commits not in it are only here.
    pub fn pushed_ref(&self) -> String {
        match self.own() {
            true => format!("origin/{}", self.head_ref),
            false => self.fetch_ref(),
        }
    }

    /// How `gh --repo` names its repository.
    pub fn repo_arg(&self) -> String {
        repo_arg(&self.host, &self.slug)
    }

    /// GitHub has another head or base for it than its worktree is on,
    /// while it is open: what it has.
    pub fn moved(&self) -> Option<&Remote> {
        (self.remote.as_ref())
            .filter(|r| r.is_open() && (r.head_sha != self.head_sha || r.base_ref != self.base_ref))
    }

    /// It was merged or closed on GitHub: how it ended.
    pub fn ended(&self) -> Option<&Remote> {
        (self.remote.as_ref()).filter(|r| !r.is_open())
    }

    /// Keep `read`, what GitHub has of it now, but what it says the same of
    /// (when that first was so stays): what changed.
    pub fn take_remote(&mut self, read: Remote) -> Change {
        match &mut self.remote {
            Some(kept) if kept.same(&read) => match kept.checks == read.checks {
                true => Change::Nothing,
                false => {
                    kept.checks = read.checks;
                    Change::Checks
                }
            },
            _ => {
                self.remote = Some(read);
                Change::Pull
            }
        }
    }

    /// GitHub's checks of the head its worktree is on, once read: none
    /// known of another head.
    pub fn checks(&self) -> Option<&[CiCheck]> {
        (self.remote.as_ref())
            .filter(|r| r.head_sha == self.head_sha)
            .map(|r| r.checks.as_slice())
    }

    /// How those checks stand, all told.
    pub fn ci(&self) -> Option<Ci> {
        self.checks().map(Ci::of)
    }

    /// Its checks' page.
    pub fn checks_url(&self) -> String {
        format!("{}/checks", self.url.trim_end_matches('/'))
    }

    /// It is on the head and base `remote` has now.
    pub fn moved_to(&mut self, head_sha: &str, base_ref: &str) {
        self.head_sha = head_sha.to_owned();
        self.base_ref = base_ref.to_owned();
    }

    /// Its head, short: `abc1234`.
    pub fn short_head(&self) -> &str {
        self.head_sha.get(..7).unwrap_or(&self.head_sha)
    }
}

impl Feature {
    /// What its reviews want goes to GitHub, as comments of the review the
    /// user submits: a review of someone else's pull request (or of one of
    /// the user's own from a fork). The user's own gets fixed by an agent.
    // shortcut: one say for the whole review; with the user's own and
    // someone else's in one, all comment (nothing is fixed here that can't
    // be pushed). Per repository once mixed reviews matter.
    pub fn posts_review(&self) -> bool {
        self.pulls().iter().any(|p| !p.works_here())
    }
}

/// A review's pull requests as a record has them: a list, or one alone, as
/// a record from before reviews held several kept it.
pub fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<PullRef>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Kept {
        Many(Vec<PullRef>),
        One(Box<PullRef>),
    }
    Ok(match Kept::deserialize(d)? {
        Kept::Many(all) => all,
        Kept::One(one) => vec![*one],
    })
}

/// The address of fork `head_repo` (`owner/repo`) of repository `slug`,
/// as origin's address `origin` names `slug`: `git@github.com:o/r.git`
/// gives `git@github.com:f/r.git`, an https one likewise.
pub fn fork_url(origin: &str, slug: &str, head_repo: &str) -> Option<String> {
    let origin = origin.trim();
    let at = origin
        .to_ascii_lowercase()
        .rfind(&slug.to_ascii_lowercase())?;
    let tail = &origin[at + slug.len()..];
    if !(tail.is_empty() || tail == ".git" || tail == "/") {
        return None;
    }
    Some(format!("{}{head_repo}{tail}", &origin[..at]))
}

/// What `gh pr view --json PUSH_FIELDS` prints.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushView {
    #[serde(default)]
    maintainer_can_modify: bool,
    #[serde(default)]
    is_cross_repository: bool,
    #[serde(default)]
    head_repository: Option<Named>,
    #[serde(default)]
    head_repository_owner: Option<Login>,
}

#[derive(Debug, Default, Deserialize)]
struct Named {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Permission {
    #[serde(default)]
    viewer_permission: String,
}

/// Whether the user can push to `pull`'s branch, read with gh in `dir`: its
/// head repository (`owner/repo`), or why not.
pub fn push_access(dir: &Path, pull: &PullRef) -> Result<Result<String, String>, String> {
    let n = pull.number.to_string();
    let arg = pull.repo_arg();
    let fields = "maintainerCanModify,isCrossRepository,headRepository,headRepositoryOwner";
    let view: PushView =
        crate::ci::gh_json(dir, &["pr", "view", &n, "--repo", &arg, "--json", fields])?;
    let owner = view
        .head_repository_owner
        .map(|o| o.login)
        .unwrap_or_default();
    let name = view.head_repository.map(|r| r.name).unwrap_or_default();
    if view.is_cross_repository {
        let me = viewer(dir, &pull.host).unwrap_or_default();
        let theirs = format!("{owner}/{name}");
        if owner.is_empty() || name.is_empty() {
            return Ok(Err(format!(
                "GitHub didn't say where #{}'s branch is",
                pull.number
            )));
        }
        return Ok(
            match view.maintainer_can_modify || owner.eq_ignore_ascii_case(&me) {
                true => Ok(theirs),
                false => Err(format!(
                    "#{} comes from {theirs}, whose author doesn't let maintainers push to it",
                    pull.number
                )),
            },
        );
    }
    let perm: Permission =
        crate::ci::gh_json(dir, &["repo", "view", &arg, "--json", "viewerPermission"])?;
    Ok(match perm.viewer_permission.as_str() {
        "ADMIN" | "MAINTAIN" | "WRITE" => Ok(pull.slug.clone()),
        _ => Err(format!("You can't push to {}", pull.slug)),
    })
}

/// The slug of pull request `number`'s review: `pr-12`.
pub fn slug_of(number: u64) -> String {
    format!("pr-{number}")
}

/// How `gh --repo` names repository `slug` of `host`: `owner/repo` on
/// github.com, `host/owner/repo` elsewhere.
pub fn repo_arg(host: &str, slug: &str) -> String {
    match host {
        "github.com" => slug.to_owned(),
        _ => format!("{host}/{slug}"),
    }
}

/// A pull request as `read` found it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub pull: PullRef,
    pub title: String,
    pub body: String,
    /// `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    pub draft: bool,
}

impl Details {
    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }
}

/// What `gh pr view --json FIELDS` prints.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct View {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    author: Option<Login>,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    #[serde(default)]
    is_cross_repository: bool,
    #[serde(default)]
    url: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    is_draft: bool,
}

#[derive(Debug, Default, Deserialize)]
struct Login {
    #[serde(default)]
    login: String,
}

/// `view` of repository `slug` of `host`, written by the user when they are
/// `me`.
fn details_of(view: View, host: &str, slug: &str, me: &str) -> Details {
    let author = view.author.map(|a| a.login).unwrap_or_default();
    let mine = !me.is_empty() && author.eq_ignore_ascii_case(me);
    let url = match view.url.is_empty() {
        true => format!("https://{host}/{slug}/pull/{}", view.number),
        false => view.url,
    };
    Details {
        pull: PullRef {
            repo: String::new(),
            host: host.to_owned(),
            slug: slug.to_owned(),
            number: view.number,
            url,
            author,
            mine,
            base_ref: view.base_ref_name,
            head_ref: view.head_ref_name,
            head_sha: view.head_ref_oid,
            cross_repo: view.is_cross_repository,
            remote: None,
            fix_here: false,
            head_repo: String::new(),
        },
        title: view.title,
        body: view.body,
        state: view.state,
        draft: view.is_draft,
    }
}

/// Read pull request `number` of repository `slug` of `host` with gh, run
/// in `dir`. Whether it is the user's own comes from who gh is signed in
/// as; when that can't be read, it is taken as someone else's, and logged.
pub fn read(dir: &Path, host: &str, slug: &str, number: u64) -> Result<Details, String> {
    let n = number.to_string();
    let arg = repo_arg(host, slug);
    let view: View =
        crate::ci::gh_json(dir, &["pr", "view", &n, "--repo", &arg, "--json", FIELDS])?;
    if view.head_ref_oid.is_empty() || view.base_ref_name.is_empty() {
        return Err(format!("gh gave no head or base of #{number}"));
    }
    let me = viewer(dir, host).unwrap_or_else(|err| {
        tracing::warn!("couldn't tell who is signed in to {host}: {err}");
        String::new()
    });
    Ok(details_of(view, host, slug, &me))
}

/// What `gh pr view --json REMOTE_FIELDS` prints.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteView {
    head_ref_oid: String,
    base_ref_name: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status_check_rollup: serde_json::Value,
}

/// How GitHub has `pull` now, read with gh in `dir` at `now`.
pub fn read_remote(dir: &Path, pull: &PullRef, now: u64) -> Result<Remote, String> {
    let n = pull.number.to_string();
    let arg = pull.repo_arg();
    let fields = "headRefOid,baseRefName,state,statusCheckRollup";
    let view: RemoteView =
        crate::ci::gh_json(dir, &["pr", "view", &n, "--repo", &arg, "--json", fields])?;
    if view.head_ref_oid.is_empty() {
        return Err(format!("gh gave no head of #{}", pull.number));
    }
    Ok(Remote {
        at: now,
        head_sha: view.head_ref_oid,
        base_ref: view.base_ref_name,
        state: view.state,
        checks: (crate::ci::rollup_checks(view.status_check_rollup).into_iter())
            .map(|c| CiCheck {
                name: c.name,
                state: c.state,
                link: c.link,
            })
            .collect(),
    })
}

/// The logins gh is signed in as, by host, once read.
static VIEWERS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Who gh is signed in to `host` as, read the first time.
fn viewer(dir: &Path, host: &str) -> Result<String, String> {
    if let Some(login) = VIEWERS.lock().ok().and_then(|v| v.get(host).cloned()) {
        return Ok(login);
    }
    let user: Login = crate::ci::gh_json(dir, &["api", "--hostname", host, "user"])?;
    if user.login.is_empty() {
        return Err("gh gave no login".into());
    }
    if let Ok(mut known) = VIEWERS.lock() {
        known.insert(host.to_owned(), user.login.clone());
    }
    Ok(user.login)
}

/// The repository of `root` whose origin is repository `slug` of `host`,
/// by name.
pub fn repo_of(root: &Root, host: &str, slug: &str) -> Option<String> {
    (root.repos.iter())
        .find(|found| {
            crate::git::web_url(&found.main)
                .as_deref()
                .and_then(crate::ci::github_repository)
                .is_some_and(|(h, s)| h.eq_ignore_ascii_case(host) && s.eq_ignore_ascii_case(slug))
        })
        .map(|found| found.name.clone())
}

/// The brief of its review: the pull request's title and description.
pub fn brief(details: &Details) -> String {
    let pull = &details.pull;
    let by = match pull.author.is_empty() {
        true => String::new(),
        false => format!(" by @{}", pull.author),
    };
    let body = details.body.trim();
    let body = match body.is_empty() {
        true => "_No description._",
        false => body,
    };
    format!(
        "# {}\n\nPull request [#{}]({}){by}: `{}` into `{}`.\n\n{body}\n",
        details.title.trim(),
        pull.number,
        pull.url,
        pull.head_ref,
        pull.base_ref,
    )
}

/// The title of its review: `#12 Fix the login`.
pub fn title(details: &Details) -> String {
    format!("#{} {}", details.pull.number, details.title.trim())
}

/// The record of the review of `details` in project `project`, its folder
/// `created` in `root`, made at `now`. The stages before `STAGES` are
/// marked done by `BY`, the bound ones on `patch`: the code's patch-id
/// once read (none leaves Build's mark for the first reading to bind, see
/// `Feature::invalidate`).
pub fn record(
    project: &str,
    root: &Root,
    created: Created,
    details: &Details,
    patch: Option<&str>,
    now: u64,
) -> Feature {
    let pull = &details.pull;
    let marks = (Stage::ALL.into_iter())
        .filter(|s| !STAGES.contains(s))
        .map(|stage| {
            let mark = Mark {
                at: now,
                by: BY.to_owned(),
                note: String::new(),
                patch: patch.filter(|_| stage.bound()).map(str::to_owned),
            };
            (stage, mark)
        })
        .collect();
    let short = pull.head_sha.get(..7).unwrap_or(&pull.head_sha);
    let mut pull = pull.clone();
    pull.repo = created
        .repos
        .first()
        .map(super::Repo::key)
        .unwrap_or_default();
    Feature {
        version: VERSION,
        kind: Kind::PrReview(vec![pull.clone()]),
        slug: pull.feature_slug(),
        title: title(details),
        project: project.to_owned(),
        created: now,
        root: root.path.clone(),
        dir: created.dir,
        multi: root.multi,
        repos: created.repos,
        marks,
        history: vec![Event {
            at: now,
            what: format!("review of #{} started at {short}", pull.number),
        }],
        ..Feature::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{Repo, Status};
    use std::path::PathBuf;

    fn view(author: &str, cross: bool) -> View {
        serde_json::from_str(&format!(
            r#"{{"number":12,"title":" Fix login ","body":"Steps","author":{{"login":"{author}"}},
               "baseRefName":"main","headRefName":"fix-login","headRefOid":"abcdef1234",
               "isCrossRepository":{cross},"url":"https://github.com/o/r/pull/12",
               "state":"OPEN","isDraft":false}}"#
        ))
        .unwrap()
    }

    #[test]
    fn reads_whose_it_is() {
        let d = details_of(view("Ann", false), "github.com", "o/r", "ann");
        assert!(d.pull.mine && d.pull.own() && d.is_open());
        assert_eq!(d.pull.branch(), "fix-login");
        assert_eq!(d.pull.pushed_ref(), "origin/fix-login");
        let fork = details_of(view("ann", true), "github.com", "o/r", "ann");
        assert!(fork.pull.mine && !fork.pull.own());
        assert_eq!(fork.pull.branch(), "gerdr/pr-12");
        let theirs = details_of(view("bob", false), "github.com", "o/r", "ann");
        assert!(!theirs.pull.mine);
        assert_eq!(theirs.pull.pushed_ref(), "refs/gerdr/pr/12");
        // Nobody known signed in: someone else's.
        assert!(
            !details_of(view("", false), "github.com", "o/r", "")
                .pull
                .mine
        );
    }

    #[test]
    fn tells_new_commits_and_its_end() {
        let mut pull = details_of(view("bob", false), "github.com", "o/r", "ann").pull;
        let read = |head: &str, base: &str, state: &str, at| Remote {
            at,
            head_sha: head.into(),
            base_ref: base.into(),
            state: state.into(),
            checks: Vec::new(),
        };
        assert_eq!(
            Change::Pull,
            pull.take_remote(read("abcdef1234", "main", "OPEN", 1))
        );
        assert_eq!(pull.moved(), None);
        // Read again the same later: kept from when it was first so.
        assert_eq!(
            Change::Nothing,
            pull.take_remote(read("abcdef1234", "main", "OPEN", 2))
        );
        assert_eq!(pull.remote.as_ref().map(|r| r.at), Some(1));
        assert_eq!(
            Change::Pull,
            pull.take_remote(read("ffff", "main", "OPEN", 3))
        );
        assert_eq!(pull.moved().map(|r| r.head_sha.as_str()), Some("ffff"));
        pull.moved_to("ffff", "main");
        assert_eq!(pull.moved(), None);
        // Retargeted is moved too.
        assert_eq!(
            Change::Pull,
            pull.take_remote(read("ffff", "dev", "OPEN", 4))
        );
        assert!(pull.moved().is_some());
        assert_eq!(
            Change::Pull,
            pull.take_remote(read("ffff", "dev", "MERGED", 5))
        );
        assert_eq!(pull.moved(), None);
        assert!(pull.ended().is_some_and(Remote::merged));
    }

    #[test]
    fn its_checks_say_how_verify_goes() {
        let check = |state, link: Option<&str>| CiCheck {
            name: "test".into(),
            state,
            link: link.map(str::to_owned),
        };
        let mut pull = details_of(view("bob", false), "github.com", "o/r", "ann").pull;
        assert_eq!(pull.ci(), None);
        let mut remote = Remote {
            at: 1,
            head_sha: "abcdef1234".into(),
            base_ref: "main".into(),
            state: "OPEN".into(),
            checks: vec![check(CiState::Running, None)],
        };
        assert_eq!(pull.take_remote(remote.clone()), Change::Pull);
        assert_eq!(pull.ci().map(|c| c.verdict()), Some(Verdict::Running));
        let run = "https://github.com/o/r/actions/runs/77/job/9";
        remote.checks = vec![
            check(CiState::Passed, None),
            check(CiState::Failed, Some(run)),
            check(
                CiState::Failed,
                Some("https://github.com/o/r/actions/runs/77/job/10"),
            ),
            check(CiState::Failed, Some("https://ci.example/build/5")),
        ];
        assert_eq!(pull.take_remote(remote.clone()), Change::Checks);
        let ci = pull.ci().unwrap();
        assert_eq!((ci.total, ci.passed, ci.failed), (4, 1, 3));
        assert_eq!(ci.verdict(), Verdict::Failed);
        assert_eq!(rerun_ids(pull.checks().unwrap()), vec![77]);
        remote.checks = vec![check(CiState::Passed, None)];
        pull.take_remote(remote.clone());
        assert_eq!(pull.ci().map(|c| c.verdict()), Some(Verdict::Passed));
        assert_eq!(Ci::of(&[]).verdict(), Verdict::None);
        // Checks of a head not reviewed say nothing of it.
        remote.head_sha = "ffff".into();
        pull.take_remote(remote);
        assert_eq!(pull.ci(), None);
        assert_eq!(pull.checks_url(), "https://github.com/o/r/pull/12/checks");
    }

    #[test]
    fn a_forks_address_follows_origins() {
        let fork = |origin| fork_url(origin, "o/r", "f/r");
        assert_eq!(
            fork("git@github.com:o/r.git").as_deref(),
            Some("git@github.com:f/r.git")
        );
        assert_eq!(
            fork("https://github.com/O/R").as_deref(),
            Some("https://github.com/f/r")
        );
        assert_eq!(
            fork("https://github.com/o/r/").as_deref(),
            Some("https://github.com/f/r/")
        );
        assert_eq!(fork("https://github.com/o/other.git"), None);
        let mut pull = details_of(view("bob", true), "github.com", "o/r", "ann").pull;
        assert!(!pull.works_here());
        pull.fix_here = true;
        pull.head_repo = "bob/r".into();
        assert!(pull.works_here());
        assert_eq!(
            pull.push_remote("git@github.com:o/r.git").as_deref(),
            Some("git@github.com:bob/r.git")
        );
        pull.cross_repo = false;
        assert_eq!(pull.push_remote("whatever").as_deref(), Some("origin"));
    }

    #[test]
    fn names_its_repository_for_gh() {
        assert_eq!(repo_arg("github.com", "o/r"), "o/r");
        assert_eq!(repo_arg("git.corp", "o/r"), "git.corp/o/r");
    }

    #[test]
    fn brief_says_what_it_is() {
        let d = details_of(view("bob", false), "github.com", "o/r", "ann");
        let text = brief(&d);
        assert!(text.starts_with("# Fix login\n"));
        assert!(text.contains("[#12](https://github.com/o/r/pull/12) by @bob"));
        assert!(text.ends_with("Steps\n"));
        assert_eq!(title(&d), "#12 Fix login");
    }

    #[test]
    fn record_starts_at_verify() {
        let d = details_of(view("bob", false), "github.com", "o/r", "ann");
        let root = Root {
            path: PathBuf::from("/r"),
            multi: false,
            repos: Vec::new(),
        };
        let created = Created {
            repos: vec![Repo {
                name: "r".into(),
                main: "/r".into(),
                base: "origin/main".into(),
                base_commit: "b".into(),
                worktree: "/r/.features/pr-12".into(),
                touched: true,
                branch: Some("gerdr/pr-12".into()),
                project: None,
            }],
            dir: "/r/.features/pr-12".into(),
            ignores: Vec::new(),
            links: Vec::new(),
        };
        let mut f = record("p", &root, created, &d, Some("p1"), 5);
        assert_eq!(f.slug, "pr-12");
        assert_eq!(f.branch(), "gerdr/pr-12");
        assert_eq!(f.current(), Some(Stage::Verify));
        assert_eq!(f.stages(), &STAGES);
        assert_eq!(f.marks[&Stage::Build].patch.as_deref(), Some("p1"));
        assert_eq!(f.marks[&Stage::Plan].patch, None);
        // New code: its author built it, so only the review goes again.
        f.tick(6, "you", Some("p1"));
        assert_eq!(f.current(), Some(Stage::AiReview));
        assert_eq!(f.invalidate("p2", 7), Some(Stage::Verify));
        assert_eq!(f.current(), Some(Stage::Verify));
        assert_eq!(f.marks[&Stage::Build].patch.as_deref(), Some("p2"));
        assert_eq!(f.stale("p2"), None);
        assert_eq!(f.status, Status::Open);
        // Saved and read back, it is still a review.
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"pr-review\""));
        let back: Feature = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pull(), f.pull());
        // A feature's record says nothing of it.
        let plain = serde_json::to_string(&Feature::default()).unwrap();
        assert!(!plain.contains("kind"));
        assert_eq!(f.pull_of("r").map(|p| p.number), Some(12));
        assert_eq!(f.pull_of("other"), None);
        // A record of one from before: one alone, unnamed, its first
        // repository's.
        let mut old = serde_json::to_value(&f).unwrap();
        let one = old["kind"]["pr-review"][0].clone();
        old["kind"]["pr-review"] = one;
        old["kind"]["pr-review"]["repo"] = serde_json::Value::Null;
        old["kind"]["pr-review"]
            .as_object_mut()
            .unwrap()
            .remove("repo");
        let read: Feature = serde_json::from_value(old).unwrap();
        assert_eq!(read.pulls().len(), 1);
        assert_eq!(read.pull_of("r").map(|p| p.number), Some(12));
    }
}
