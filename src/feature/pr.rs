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
}

impl PullRef {
    /// The user's own, from a branch of this repository: gerdr works on
    /// that branch, and its fixes can be pushed to it.
    pub fn own(&self) -> bool {
        self.mine && !self.cross_repo
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
    Feature {
        version: VERSION,
        kind: Kind::PrReview(pull.clone()),
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
    }
}
