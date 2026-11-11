//! Shipping a feature, and what stops it.
//!
//! `probe` reads a feature for the panel, off the UI thread: its patch-id,
//! its guards, and when Ship is its stage, what can stop each step of it
//! (`ShipCheck`). Which steps run is the page's ticks (`ShipPlan`), so
//! `plan_actions` says what shipping does from the two, with no new probe.
//! Each repository with commits goes its own way (`Route`):
//!
//! - through origin (`run_steps`), in the feature's order (home project
//!   first, then each link): allow the push (`hook::allow`), push
//!   `feature/<slug>` with its upstream, guard it again, and open a pull
//!   request into its base with `gh pr create`, or keep the one open for the
//!   branch already. With more than one, every body links the others
//!   (`gh pr edit`).
//! - squash (`squash_command`): a command for a tab that goes to each
//!   repository's own main worktree in turn (a linked project's too) and
//!   squashes the branch into the base and commits, the message
//!   prefilled with the title, for the user to edit. When ticked,
//!   `feature/<slug>` goes to origin first, as through origin. The feature is done
//!   once every such branch reads as merged (`Probe::merged`), then those
//!   ticked go to origin (`push_base`).
//! - nothing: it stays as it is. With no step ticked anywhere, the feature
//!   finishes without shipping, once the user says so.
//!
//! Runs git and gh: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use super::{
    Feature, PerProject, Pr, Repo, RepoPlan, Route, ShipDone, ShipMode, ShipStep, Stage, Status,
    hook::{self, Guard},
    layout::{git, merged_into_base, shown},
    page,
    patch::{self, Reading},
};
use crate::{git as gitinfo, git_ops};

/// One step of shipping, in the order they run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Push `feature/<slug>` of `repo` to origin.
    Push { repo: String },
    /// Open a pull request of `repo` into `base`.
    OpenPr { repo: String, base: String },
    /// Keep `repo`'s pull request open for the branch already: the push
    /// updates it.
    KeepPr { repo: String, url: String },
    /// Link the pull requests to each other in their bodies.
    LinkPrs,
    /// Squash `feature/<slug>` of `repo` into `base` in its main worktree.
    Squash { repo: String, base: String },
    /// Push `base` of `repo` to origin, once squashed.
    PushBase { repo: String, base: String },
}

impl Action {
    /// In words, for the Ship sheet, as the Ship page's rows say it:
    /// "Push feature/x to origin", "Open a pull request into dev". With
    /// several repositories (`one` false) each names its own: "Push
    /// feature/x of services-api to origin".
    pub fn label(&self, branch: &str, one: bool) -> String {
        let named = |word: &str, repo: &str| match one {
            true => String::new(),
            false => format!(" {word} {repo}"),
        };
        match self {
            Self::Push { repo } => format!("Push {branch}{} to origin", named("of", repo)),
            Self::OpenPr { repo, base } => {
                format!("Open a pull request into {base}{}", named("for", repo))
            }
            Self::KeepPr { repo, url } => {
                format!("Keep the pull request open{}: {url}", named("for", repo))
            }
            Self::LinkPrs => "Link the pull requests to each other".into(),
            Self::Squash { repo, base } => {
                format!("Squash {branch} into {base}{}", named("in", repo))
            }
            Self::PushBase { repo, base } => {
                format!("Push {base}{} to origin", named("of", repo))
            }
        }
    }
}

/// What can stop a feature shipping, however its ticks stand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipCheck {
    /// What stops any shipping, one line each ("services-api has uncommitted
    /// changes"). Finishing without shipping goes on regardless.
    pub blockers: Vec<String>,
    /// The repositories it can ship: those with commits, not merged yet.
    pub repos: Vec<RepoCheck>,
    /// What is worth knowing but stops nothing ("aft-backend has no
    /// commits: left out").
    pub notes: Vec<String>,
    /// Its code couldn't be read: nothing is known of what it would do,
    /// not even that it would do nothing.
    pub unread: bool,
}

/// A repository `ShipCheck` found to ship.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoCheck {
    /// Its `Repo::key`.
    pub key: String,
    /// The branch it merges into.
    pub base: String,
    /// Its main worktree.
    pub main: PathBuf,
    /// Its commits on the feature's branch.
    pub commits: usize,
    /// The route it takes without a pick (`mode_of`).
    pub route: Route,
    /// Why it can't go through origin, if so.
    pub no_origin: Option<String>,
    /// Why it can't merge into its base in its main worktree, if so.
    pub no_merge: Option<String>,
    /// Its branch reads as merged into its base already: by a squash of
    /// the feature's, or its pull request merged on GitHub.
    pub merged: bool,
    /// The commit its worktree is on, if read.
    pub head: Option<String>,
    /// The commit origin's `feature/<slug>` was at when last fetched, if
    /// known.
    pub pushed: Option<String>,
}

impl RepoCheck {
    /// Why `step` can't run, if so.
    pub fn why_not(&self, step: ShipStep) -> Option<&str> {
        match step {
            ShipStep::Push | ShipStep::OpenPr | ShipStep::PushBranch | ShipStep::PushBase => {
                self.no_origin.as_deref()
            }
            ShipStep::Merge => self.no_merge.as_deref(),
        }
    }

    /// Its squash waits for its base to be pulled: its branch took in
    /// commits of origin's base that the base here lacks (`pull_base`).
    pub fn pulls_base(&self) -> bool {
        (self.no_merge.as_deref()).is_some_and(|why| why.ends_with(&pull_first(&self.base)))
    }
}

/// How the squash's blocker ends when `base` lacks what origin's has.
fn pull_first(base: &str) -> String {
    format!(": pull {base} first")
}

/// Move `repo`'s base on to origin's in its main worktree, as a pull does,
/// only fast-forward: what its squash waits for (`RepoCheck::pulls_base`).
/// How many commits it took.
pub fn pull_base(repo: &Repo) -> Result<usize, String> {
    let remote = format!("origin/{}", repo.base);
    let range = format!("{}..{remote}", repo.base);
    let count = git(&repo.main, &["rev-list", "--count", &range])?;
    let n = (count.trim().parse()).map_err(|err| format!("couldn't read a commit count: {err}"))?;
    git(&repo.main, &["merge", "-q", "--ff-only", &remote])?;
    Ok(n)
}

/// What the ticks ask of a feature, given its `ShipCheck`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Planned {
    /// What shipping does, in order; empty while `blockers` has any.
    pub actions: Vec<Action>,
    /// What stops what is ticked.
    pub blockers: Vec<String>,
    /// No step is ticked: shipping would do nothing.
    pub nothing: bool,
    /// Steps are ticked, but every one of them ran already, on the code
    /// as it is now: there is nothing left to run.
    pub all_done: bool,
}

impl Planned {
    /// How many of its actions run a step: keeping a pull request open and
    /// linking them come with those.
    pub fn steps(&self) -> usize {
        (self.actions.iter())
            .filter(|a| !matches!(a, Action::KeepPr { .. } | Action::LinkPrs))
            .count()
    }
}

/// Whether `step` of `repo` ran already and needn't again: a push only while
/// the branch is where it was pushed from, a squash only while it reads as
/// merged.
pub fn step_done(feature: &Feature, repo: &RepoCheck, step: ShipStep) -> Option<super::ShipDone> {
    let done = feature.ship_done(&repo.key, step)?;
    let holds = match step {
        // A push from before the log kept its commit holds while origin
        // has the branch where the worktree is.
        ShipStep::Push | ShipStep::PushBranch => match done.sha.as_deref() {
            Some(sha) => Some(sha) == repo.head.as_deref(),
            None => repo.pushed.is_some() && repo.pushed == repo.head,
        },
        ShipStep::Merge => repo.merged,
        ShipStep::OpenPr | ShipStep::PushBase => true,
    };
    holds.then_some(done)
}

/// What `repo` of `feature` does, by the page's ticks.
pub fn repo_plan(feature: &Feature, repo: &RepoCheck) -> RepoPlan {
    feature.ship_plan.repo(&repo.key, repo.route)
}

/// What `feature`'s ticks ask, as `check` found it. Runs nothing: the page
/// calls it as ticks change.
pub fn plan_actions(feature: &Feature, check: &ShipCheck) -> Planned {
    let mut out = Planned::default();
    if check.unread {
        out.blockers = check.blockers.clone();
        return out;
    }
    let plans: Vec<(&RepoCheck, RepoPlan)> = (check.repos.iter())
        .map(|repo| (repo, repo_plan(feature, repo)))
        .collect();
    out.nothing = plans.iter().all(|(_, plan)| plan.steps().is_empty());
    if out.nothing {
        return out;
    }
    out.blockers = check.blockers.clone();
    let mut through_origin = Vec::new();
    let mut squashes = Vec::new();
    let mut prs = 0;
    let mut fresh = 0;
    let mut new_prs = 0;
    for (repo, plan) in &plans {
        for step in plan.steps() {
            // What ran stays: only what is left runs. A pull request kept
            // open still counts for linking the others to it.
            if let Some(done) = step_done(feature, repo, step) {
                if let (ShipStep::OpenPr, Some(url)) = (step, done.url) {
                    prs += 1;
                    through_origin.push(Action::KeepPr {
                        repo: repo.key.clone(),
                        url,
                    });
                }
                continue;
            }
            // Merged, only its base is left to push, once its squash ran.
            let squashed = feature.ship_done(&repo.key, ShipStep::Merge).is_some();
            if repo.merged && !(step == ShipStep::PushBase && squashed) {
                continue;
            }
            fresh += 1;
            if let Some(why) = repo.why_not(step) {
                let why = why.to_owned();
                if !out.blockers.contains(&why) {
                    out.blockers.push(why);
                }
                continue;
            }
            let (key, base) = (repo.key.clone(), repo.base.clone());
            match step {
                ShipStep::Push | ShipStep::PushBranch => {
                    through_origin.push(Action::Push { repo: key })
                }
                ShipStep::OpenPr => {
                    prs += 1;
                    new_prs += 1;
                    let kept = feature.shipping.prs.iter().find(|p| p.repo == key);
                    through_origin.push(match kept {
                        Some(pr) => Action::KeepPr {
                            url: pr.url.clone(),
                            repo: key,
                        },
                        None => Action::OpenPr { repo: key, base },
                    });
                }
                ShipStep::Merge => squashes.push(Action::Squash { repo: key, base }),
                ShipStep::PushBase => squashes.push(Action::PushBase { repo: key, base }),
            }
        }
    }
    if !out.blockers.is_empty() {
        return out;
    }
    if fresh == 0 {
        out.all_done = true;
        return out;
    }
    out.actions = through_origin;
    // Those linked already stay as they are.
    if prs >= 2 && new_prs > 0 {
        out.actions.push(Action::LinkPrs);
    }
    out.actions.extend(squashes);
    out
}

/// Whether the repositories with work all merge by `feature`'s ticks, which
/// makes removing its worktrees and branches safe to ask for ahead.
pub fn removable(feature: &Feature, check: &ShipCheck) -> bool {
    !check.unread
        && check.repos.iter().all(|repo| {
            let plan = repo_plan(feature, repo);
            repo.merged || (plan.route == Route::Merge && plan.merge)
        })
}

/// What `probe` read of a feature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// Its patch-id and uncommitted changes, or why they couldn't be read.
    pub reading: Result<Reading, String>,
    /// Per touched repository, by `Repo::key`: whether pushes from its
    /// worktree are guarded (after making sure of the hook there).
    pub guards: BTreeMap<String, Guard>,
    /// When Ship is its current stage: what can stop it shipping.
    pub ship: Option<ShipCheck>,
    /// When a squash started (`Shipping::squash_at`): whether every
    /// touched repository with commits reads as merged into its base.
    pub merged: bool,
    /// Per touched repository, by `Repo::key`: the files and lines it
    /// changes against its base, uncommitted ones included; none for one
    /// that couldn't be read (see `page::stats`).
    pub stats: BTreeMap<String, page::Stat>,
    /// Per fix round with its code read before its fixes, by number: what
    /// its fixes changed in each repository (see `page::round_stats`).
    pub rounds: BTreeMap<u32, Result<Vec<page::RoundStat>, String>>,
    /// The stage files in its data dir, and their milestones.
    pub docs: page::Docs,
    /// The models its stages' agents use, with the run each read was of
    /// (`agents::read_models`).
    pub models: BTreeMap<Stage, (u32, String)>,
    /// Its pull requests as GitHub has them, by `Repo::key`, when read this
    /// time (`read_prs`): read less often than the rest.
    pub prs: Option<BTreeMap<String, Result<PrState, String>>>,
    /// How its repositories stand against their bases (`read_bases`), when
    /// it can still ship.
    pub bases: BTreeMap<String, BaseState>,
    /// How the conflicts its agent takes on stand (`read_resolution`).
    pub resolution: Option<Resolution>,
}

/// A pull request as GitHub has it now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrState {
    pub number: u64,
    /// `OPEN`, `MERGED` or `CLOSED`.
    pub state: String,
    pub draft: bool,
    /// Its checks: passed, failed and running ones.
    pub passed: usize,
    pub failed: usize,
    pub running: usize,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or empty.
    pub review: String,
    /// `MERGEABLE`, `CONFLICTING` or `UNKNOWN`.
    pub mergeable: String,
}

impl PrState {
    /// How it stands in a few words: "open \u{b7} 2 checks passed \u{b7}
    /// approved".
    pub fn words(&self) -> String {
        let mut parts = vec![match self.state.as_str() {
            "MERGED" => "merged".to_owned(),
            "CLOSED" => "closed".to_owned(),
            _ if self.draft => "draft".to_owned(),
            _ => "open".to_owned(),
        }];
        if self.state == "OPEN" {
            let checks = |n: usize, what: &str| match n {
                1 => format!("1 check {what}"),
                n => format!("{n} checks {what}"),
            };
            if self.failed > 0 {
                parts.push(checks(self.failed, "failed"));
            } else if self.running > 0 {
                parts.push(checks(self.running, "running"));
            } else if self.passed > 0 {
                parts.push(checks(self.passed, "passed"));
            }
            match self.review.as_str() {
                "APPROVED" => parts.push("approved".into()),
                "CHANGES_REQUESTED" => parts.push("changes asked".into()),
                "REVIEW_REQUIRED" => parts.push("review required".into()),
                _ => {}
            }
            if self.mergeable == "CONFLICTING" {
                parts.push("conflicts with its base".into());
            }
        }
        parts.join(" \u{b7} ")
    }
}

/// Read `feature`'s pull requests on GitHub with `gh`, by `Repo::key`: those
/// shipping opened, of repositories whose worktree is still there.
pub fn read_prs(feature: &Feature, gh: &str) -> BTreeMap<String, Result<PrState, String>> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct View {
        #[serde(default)]
        number: u64,
        #[serde(default)]
        state: String,
        #[serde(default)]
        is_draft: bool,
        #[serde(default)]
        mergeable: String,
        #[serde(default)]
        review_decision: Option<String>,
        #[serde(default)]
        status_check_rollup: serde_json::Value,
    }
    let mut out = BTreeMap::new();
    for repo in feature.touched().filter(|r| r.worktree.is_dir()) {
        let Some(url) = (feature.ship_done(&repo.key(), ShipStep::OpenPr)).and_then(|d| d.url)
        else {
            continue;
        };
        let fields = "number,state,isDraft,mergeable,reviewDecision,statusCheckRollup";
        let read = run_gh(gh, &repo.worktree, &["pr", "view", &url, "--json", fields])
            .and_then(|out| serde_json::from_str::<View>(&out).map_err(|e| e.to_string()))
            .map(|view| {
                let states = crate::ci::rollup_states(view.status_check_rollup);
                let count = |s| states.iter().filter(|x| **x == s).count();
                PrState {
                    number: view.number,
                    state: view.state,
                    draft: view.is_draft,
                    passed: count(crate::ci::CiState::Passed),
                    failed: count(crate::ci::CiState::Failed),
                    running: count(crate::ci::CiState::Running),
                    review: view.review_decision.unwrap_or_default(),
                    mergeable: view.mergeable,
                }
            });
        if let Err(why) = &read {
            tracing::warn!("reading the pull request {url}: {why}");
        }
        out.insert(repo.key(), read);
    }
    out
}

/// Each project's ship mode setting, if it has one (`Workflow::ship`).
pub type Modes = PerProject<Option<ShipMode>>;

/// How `feature`'s repository `repo` (its `Repo::key`) ships: the mode its
/// own project sets in `modes`, if any, else a pull request when its
/// `origin` is on GitHub, else a squash.
pub fn mode_of(feature: &Feature, repo: &str, modes: &Modes) -> ShipMode {
    let found = feature.repo(repo);
    if let Some(mode) = found.and_then(|r| modes.of(r)).copied().flatten() {
        return mode;
    }
    let on_github = found
        .and_then(|r| gitinfo::web_url(&r.main))
        .is_some_and(|url| url.starts_with("https://github.com/"));
    if on_github {
        ShipMode::Pr
    } else {
        ShipMode::Squash
    }
}

/// Read `feature` for the panel (see `Probe`), but its stage files (the
/// caller reads them, `page::read_docs`); `modes` are its projects'
/// settings. Makes sure of every touched repository's hook and guard file
/// on the way (`hook::ensure`, `hook::guard_if_missing`).
pub fn probe(feature: &Feature, modes: &Modes) -> Probe {
    let reading = patch::read(feature);
    let branch = feature.branch();
    // The user's own pull request's branch is theirs to push from anywhere.
    let guards = (feature.touched())
        .filter(|repo| !feature.pull_of(&repo.key()).is_some_and(super::pr::PullRef::own))
        .map(|repo| {
            let branch = repo.branch.clone().unwrap_or_else(|| branch.clone());
            (repo.key(), guard_of(repo, &branch))
        })
        .collect();
    // A pull request's review submits it instead (`pr_post`).
    let ship = (feature.ships() && feature.pull().is_none()).then(|| check(feature, &reading, modes));
    let merged =
        feature.shipping.squash_at.is_some() && reading.as_ref().is_ok_and(|r| merged(feature, r));
    Probe {
        reading,
        guards,
        ship,
        merged,
        stats: page::stats(feature.touched().filter(|r| r.worktree.is_dir())),
        rounds: page::round_stats(feature),
        docs: page::Docs::default(),
        models: BTreeMap::new(),
        prs: None,
        bases: BTreeMap::new(),
        resolution: None,
    }
}

/// Read `feature` written on another machine, as much as is read of it here: its
/// code as far as it came. Its hooks aren't made sure of: nothing is pushed from
/// here, and Ship is for its writer.
pub fn probe_seen(feature: &Feature) -> Probe {
    Probe {
        reading: patch::read(feature),
        guards: BTreeMap::new(),
        ship: None,
        merged: false,
        stats: page::stats(feature.touched().filter(|r| r.worktree.is_dir())),
        rounds: page::round_stats(feature),
        docs: page::Docs::default(),
        models: BTreeMap::new(),
        prs: None,
        bases: BTreeMap::new(),
        resolution: None,
    }
}

/// Read finished `feature` for its tab: the line counts of its touched
/// repositories whose worktree is still there, and nothing of its code
/// (`reading` says so; a finished feature's tab doesn't show it).
pub fn probe_finished(feature: &Feature) -> Probe {
    Probe {
        reading: Err(format!(
            "feature is {}: its code isn't read",
            status_word(feature.status)
        )),
        guards: BTreeMap::new(),
        ship: None,
        merged: false,
        stats: page::stats(feature.touched().filter(|r| r.worktree.is_dir())),
        rounds: page::round_stats(feature),
        docs: page::Docs::default(),
        models: BTreeMap::new(),
        prs: None,
        bases: BTreeMap::new(),
        resolution: None,
    }
}

/// How a feature's `status` reads in a sentence.
fn status_word(status: Status) -> &'static str {
    match status {
        Status::Open => "open",
        Status::Done => "done",
        Status::Abandoned => "abandoned",
    }
}

/// Whether pushes of `branch` from `repo`'s worktree are guarded, making
/// sure of its hook and guard file.
fn guard_of(repo: &Repo, branch: &str) -> Guard {
    if !repo.worktree.is_dir() {
        return Guard::Off(format!(
            "its worktree {} isn't there",
            shown(&repo.worktree)
        ));
    }
    if let Err(err) = hook::guard_if_missing(&repo.main, branch) {
        return Guard::Off(err);
    }
    hook::ensure(&repo.worktree)
}

/// Whether the repositories squashing have `feature`'s branch merged into
/// their bases: those of `Shipping::squash_repos`, or on a record from
/// before it was kept, every touched repository with commits.
fn merged(feature: &Feature, reading: &Reading) -> bool {
    let branch = feature.branch();
    let squashing = &feature.shipping.squash_repos;
    let mut shipped = feature
        .touched()
        .filter(|r| match squashing.is_empty() {
            true => reading.commits.get(&r.key()).is_some_and(|n| *n > 0),
            false => squashing.contains(&r.key()),
        })
        .peekable();
    if shipped.peek().is_none() {
        return false;
    }
    shipped.all(|repo| merged_into_base(repo, &branch))
}

/// What can stop `feature`, at Ship, shipping as `reading` found it.
fn check(feature: &Feature, reading: &Result<Reading, String>, modes: &Modes) -> ShipCheck {
    let mut out = ShipCheck::default();
    for stage in Stage::ALL.into_iter().filter(|s| *s < Stage::Ship) {
        if !feature.is_done(stage) {
            out.blockers.push(format!("{} isn't done", stage.label()));
        }
    }
    let open = feature.review.as_ref().map_or(0, |r| r.open());
    match open {
        0 => {}
        1 => out.blockers.push("1 AI review finding open".into()),
        n => out.blockers.push(format!("{n} AI review findings open")),
    }
    let reading = match reading {
        Ok(reading) => reading,
        Err(err) => {
            out.blockers.push(format!("Couldn't read the code: {err}"));
            out.unread = true;
            return out;
        }
    };
    if let Some(stage) = feature.stale(&reading.patch).filter(|s| *s < Stage::Ship) {
        out.blockers
            .push(format!("Code changed since {} was marked", stage.label()));
    }
    for name in &reading.dirty {
        out.blockers.push(format!("{name} has uncommitted changes"));
    }
    for name in &reading.stray {
        out.blockers.push(format!(
            "{name} has changes, but the feature doesn't change it"
        ));
    }
    let branch = feature.branch();
    for repo in feature.touched() {
        let commits = reading.commits.get(&repo.key()).copied().unwrap_or(0);
        if commits == 0 {
            out.notes
                .push(format!("{} has no commits: left out", repo.key()));
            continue;
        }
        let merged = merged_into_base(repo, &branch);
        out.repos.push(RepoCheck {
            key: repo.key(),
            base: repo.base.clone(),
            main: repo.main.clone(),
            commits,
            route: Route::of_mode(mode_of(feature, &repo.key(), modes)),
            no_origin: git(&repo.main, &["remote", "get-url", "origin"])
                .is_err()
                .then(|| format!("{} has no origin remote", repo.key())),
            no_merge: (!merged).then(|| squash_blocker(repo, &branch)).flatten(),
            merged,
            head: reading.heads.get(&repo.key()).cloned(),
            pushed: tracking(repo, &branch),
        });
    }
    out
}

/// The commit origin's `branch` of `repo` was at when last fetched.
fn tracking(repo: &Repo, branch: &str) -> Option<String> {
    let name = format!("refs/remotes/origin/{branch}");
    let sha = git(&repo.main, &["rev-parse", "-q", "--verify", &name]).ok()?;
    Some(sha.trim().to_owned())
}

/// What stops squashing `branch` into `repo`'s base in its main worktree.
fn squash_blocker(repo: &Repo, branch: &str) -> Option<String> {
    let (name, base) = (&repo.key(), &repo.base);
    let on = git(&repo.main, &["symbolic-ref", "-q", "--short", "HEAD"])
        .map(|b| b.trim().to_owned())
        .unwrap_or_default();
    if on.is_empty() {
        return Some(format!(
            "{name}'s main worktree is on no branch: switch it to {base}"
        ));
    }
    if &on != base {
        return Some(format!(
            "{name}'s main worktree is on {on}: switch it to {base}"
        ));
    }
    match git(
        &repo.main,
        &["status", "--porcelain", "--untracked-files=no"],
    ) {
        Ok(status) if !status.trim().is_empty() => {
            return Some(format!(
                "{name}'s main worktree has uncommitted changes on {base}"
            ));
        }
        Ok(_) => {}
        Err(err) => {
            return Some(format!(
                "{name}: couldn't tell what its main worktree has: {err}"
            ));
        }
    }
    // Taken in from origin's base, its commits would go into the squash
    // when the local base lacks them: as the feature's own, once pushed.
    let remote = format!("origin/{base}");
    let taken = git(&repo.main, &["merge-base", branch, &remote])
        .ok()
        .map(|at| at.trim().to_owned())
        .filter(|at| !at.is_empty())
        .and_then(|at| git(&repo.main, &["rev-list", "--count", &at, "--not", base]).ok())
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if taken > 0 {
        let commits = match taken {
            1 => "a commit".to_owned(),
            n => format!("{n} commits"),
        };
        return Some(format!(
            "{name}: {branch} has {commits} of {remote} that {base} lacks{}",
            pull_first(base)
        ));
    }
    match git_ops::predict_merge(&repo.main, base, branch) {
        git_ops::Verdict::Clean => None,
        git_ops::Verdict::Conflicts(files) => Some(format!(
            "{name}: squashing into {base} conflicts in {}",
            files.join(", ")
        )),
        git_ops::Verdict::Blocked(why) => Some(format!(
            "{name}: couldn't tell whether squashing into {base} conflicts: {why}"
        )),
        git_ops::Verdict::Unmerged(_) => None,
    }
}

/// Run `actions` of a pull-request ship of `feature`, with `gh` the
/// program to run as gh. Returns the pull requests, or why it stopped with
/// those opened by then: running it again goes on from there.
///
/// Every push is allowed right before it (`hook::allow`) and guarded again
/// after, whether it went or not.
#[cfg(test)]
pub fn open_prs(
    feature: &Feature,
    actions: &[Action],
    gh: &str,
) -> Result<Vec<Pr>, (String, Vec<Pr>)> {
    let (ran, failed) = run_steps(feature, actions, gh, super::now());
    match failed {
        None => Ok(ran.prs),
        Some(why) => Err((why, ran.prs)),
    }
}

/// What `run_steps` did: each step it ran, and the pull requests it opened or
/// kept, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    pub done: Vec<ShipDone>,
    pub prs: Vec<Pr>,
}

/// Run `actions` of `feature` that go through origin, at unix seconds
/// `now` (see `open_prs`): what ran, and why it stopped if it did. A
/// `PushBase` here is of a repository merged already, as no squash of it
/// comes first; one after a squash waits for it and isn't run here.
pub fn run_steps(
    feature: &Feature,
    actions: &[Action],
    gh: &str,
    now: u64,
) -> (Ran, Option<String>) {
    let branch = feature.branch();
    let mut ran = Ran::default();
    let repo_of = |key: &str| feature.repo(key);
    let squashing = |key: &str| {
        (actions.iter()).any(|a| matches!(a, Action::Squash { repo, .. } if repo == key))
    };
    let done = |repo: &str, step, sha: Option<String>, url: Option<String>| ShipDone {
        at: now,
        repo: repo.to_owned(),
        step,
        sha,
        url,
        undone: None,
    };
    for action in actions {
        let missing = || {
            format!(
                "{} has no repository {}",
                feature.title,
                action_repo(action)
            )
        };
        match action {
            Action::Push { repo } => {
                let Some(r) = repo_of(repo) else {
                    return (ran, Some(missing()));
                };
                match push(r, &branch, feature.rebased_since_push(repo)) {
                    Ok(sha) => ran.done.push(done(repo, ShipStep::Push, Some(sha), None)),
                    Err(why) => {
                        return (
                            ran,
                            Some(format!("Couldn't push {branch} of {repo}: {why}")),
                        );
                    }
                }
            }
            Action::KeepPr { repo, url } => ran.prs.push(Pr {
                repo: repo.clone(),
                url: url.clone(),
            }),
            Action::OpenPr { repo, base } => {
                let Some(r) = repo_of(repo) else {
                    return (ran, Some(missing()));
                };
                match open_pr(feature, r, base, &branch, gh) {
                    Ok(url) => {
                        ran.done
                            .push(done(repo, ShipStep::OpenPr, None, Some(url.clone())));
                        ran.prs.push(Pr {
                            repo: repo.clone(),
                            url,
                        });
                    }
                    Err(why) => {
                        let why = format!("Couldn't open a pull request of {repo}: {why}");
                        return (ran, Some(why));
                    }
                }
            }
            Action::LinkPrs => {
                if ran.prs.len() < 2 {
                    continue;
                }
                for pr in &ran.prs {
                    let Some(r) = repo_of(&pr.repo) else { continue };
                    let mut body = format!("{}\n\nThe others:\n", pr_body(feature));
                    for other in ran.prs.iter().filter(|o| o.url != pr.url) {
                        body.push_str(&format!("- {}: {}\n", other.repo, other.url));
                    }
                    let args = ["pr", "edit", pr.url.as_str(), "--body", body.as_str()];
                    if let Err(why) = run_gh(gh, &r.worktree, &args) {
                        let why = format!("Couldn't link the pull request of {}: {why}", pr.repo);
                        return (ran, Some(why));
                    }
                }
            }
            Action::PushBase { repo, base } if !squashing(repo) => {
                let Some(r) = repo_of(repo) else {
                    return (ran, Some(missing()));
                };
                match push_base(r) {
                    Ok(sha) => ran
                        .done
                        .push(done(repo, ShipStep::PushBase, Some(sha), None)),
                    Err(why) => {
                        return (ran, Some(format!("Couldn't push {base} of {repo}: {why}")));
                    }
                }
            }
            Action::Squash { .. } | Action::PushBase { .. } => {}
        }
    }
    (ran, None)
}

/// How a repository's branch stands against its base on origin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseState {
    /// What it is held against: `origin/main`, else the local `main`.
    pub onto: String,
    /// The base's commits not in the branch.
    pub behind: usize,
    /// The files that conflict taking them in; none when it goes cleanly.
    pub conflicts: Vec<String>,
    /// Why it couldn't be told whether it conflicts, or fetched.
    pub why: Option<String>,
    /// The branch took its base in by merges before: a rebase would play
    /// its commits again one by one, and stop where those merges settled
    /// conflicts.
    pub merges: bool,
}

impl BaseState {
    /// It can take its base in by itself: behind, with no conflict known.
    pub fn clean(&self) -> bool {
        self.behind > 0 && self.conflicts.is_empty() && self.why.is_none()
    }
}

/// How each of `feature`'s repositories stands against its base, by
/// `Repo::key`: those with a worktree, not merged. `fetch`: ask origin
/// for the base first (network: not every read).
pub fn read_bases(feature: &Feature, fetch: bool) -> BTreeMap<String, BaseState> {
    let branch = feature.branch();
    let mut out = BTreeMap::new();
    for repo in feature.touched().filter(|r| r.worktree.is_dir()) {
        if merged_into_base(repo, &branch) {
            continue;
        }
        let dir = &repo.worktree;
        let mut state = BaseState::default();
        if fetch && let Err(failed) = git_ops::fetch_branch(dir, "origin", &repo.base) {
            state.why = Some(format!("couldn't fetch {}: {}", repo.base, failed.why));
        }
        let remote = format!("origin/{}", repo.base);
        let has = |r: &str| git(dir, &["rev-parse", "--verify", "-q", r]).is_ok();
        state.onto = if has(&remote) {
            remote
        } else {
            repo.base.clone()
        };
        let range = format!("HEAD..{}", state.onto);
        state.behind = git(dir, &["rev-list", "--count", &range])
            .ok()
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
        let mine = format!("{}..HEAD", state.onto);
        state.merges = git(dir, &["rev-list", "--merges", "--count", &mine])
            .ok()
            .and_then(|n| n.trim().parse::<usize>().ok())
            .is_some_and(|n| n > 0);
        if state.behind > 0 {
            match git_ops::predict_merge(dir, "HEAD", &state.onto) {
                git_ops::Verdict::Clean | git_ops::Verdict::Unmerged(_) => {}
                git_ops::Verdict::Conflicts(files) => state.conflicts = files,
                git_ops::Verdict::Blocked(why) => {
                    state
                        .why
                        .get_or_insert(format!("couldn't tell what conflicts: {why}"));
                }
            }
        }
        out.insert(repo.key(), state);
    }
    out
}

/// Take `onto` into repository `key`'s branch of `feature`, `way`, in its
/// worktree: what it was, what it is now, and the commit taken in; the
/// feature's patch-id after, if it can be read. A merge or rebase that
/// stops on conflicts is aborted, leaving all as it was.
pub fn update_from_base(
    feature: &Feature,
    key: &str,
    way: super::UpdateWay,
    now: u64,
) -> Result<(super::BaseUpdate, Option<String>), String> {
    let repo =
        (feature.repo(key)).ok_or_else(|| format!("{} has no repository {key}", feature.title))?;
    let dir = &repo.worktree;
    let status = git(dir, &["status", "--porcelain", "--untracked-files=no"])?;
    if !status.trim().is_empty() {
        return Err(format!("{key} has uncommitted changes: commit them first"));
    }
    let remote = format!("origin/{}", repo.base);
    let onto = match git(dir, &["rev-parse", "--verify", "-q", &remote]) {
        Ok(_) => remote,
        Err(_) => repo.base.clone(),
    };
    let before = git(dir, &["rev-parse", "HEAD"])?.trim().to_owned();
    let onto_sha = git(dir, &["rev-parse", &onto])?.trim().to_owned();
    let how = match way {
        super::UpdateWay::Merge => git_ops::Integrate::Merge,
        super::UpdateWay::Rebase => git_ops::Integrate::Rebase,
    };
    if let Err(failed) = git_ops::integrate(dir, &onto, how) {
        // Not stopped part way, say on one the user had going: theirs stays.
        let own = matches!(failed.failure, git_ops::Failure::Conflict(_) | git_ops::Failure::Timeout);
        if !own {
            return Err(format!("{key}: couldn't take {onto} in: {}", failed.why));
        }
        // Stopped part way: back to where it was.
        let stopped = match way {
            super::UpdateWay::Merge => crate::git::InProgress::Merge,
            super::UpdateWay::Rebase => crate::git::InProgress::Rebase { step: 0, total: 0 },
        };
        if let Err(left) = git_ops::abort_op(dir, stopped) {
            tracing::warn!("aborting the update of {key}: {}", left.why);
            return Err(format!(
                "{key}: couldn't take {onto} in: {}; nor stop it: {}",
                failed.why, left.why
            ));
        }
        return Err(format!("{key}: couldn't take {onto} in: {}", failed.why));
    }
    let after = git(dir, &["rev-parse", "HEAD"])?.trim().to_owned();
    let patch = patch::read(feature).ok().map(|r| r.patch);
    let update = super::BaseUpdate {
        at: now,
        repo: key.to_owned(),
        way,
        onto,
        onto_sha,
        before,
        after,
    };
    Ok((update, patch))
}

/// Where a merge for an agent to resolve starts: what it takes in and the
/// branch's HEAD before it, and the files it stopped on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeBegun {
    pub onto: String,
    pub onto_sha: String,
    pub before: String,
    /// Empty: it merged cleanly, with nothing to resolve.
    pub files: Vec<String>,
}

/// What repository `key`'s branch of `feature` would take in from its base:
/// `origin/<base>` if there, else the local base; and its HEAD.
fn merge_ends(feature: &Feature, key: &str) -> Result<(PathBuf, String, String, String), String> {
    let repo =
        (feature.repo(key)).ok_or_else(|| format!("{} has no repository {key}", feature.title))?;
    let dir = repo.worktree.clone();
    let remote = format!("origin/{}", repo.base);
    let onto = match git(&dir, &["rev-parse", "--verify", "-q", &remote]) {
        Ok(_) => remote,
        Err(_) => repo.base.clone(),
    };
    let onto_sha = git(&dir, &["rev-parse", &onto])?.trim().to_owned();
    let before = git(&dir, &["rev-parse", "HEAD"])?.trim().to_owned();
    Ok((dir, onto, onto_sha, before))
}

/// Start merging repository `key`'s base into `feature`'s branch, in its
/// worktree, for an agent to resolve what it stops on.
pub fn begin_conflict_merge(feature: &Feature, key: &str) -> Result<MergeBegun, String> {
    let (dir, onto, onto_sha, before) = merge_ends(feature, key)?;
    let status = git(&dir, &["status", "--porcelain", "--untracked-files=no"])?;
    if !status.trim().is_empty() {
        return Err(format!("{key} has uncommitted changes: commit them first"));
    }
    let files = git_ops::begin_merge(&dir, &onto)
        .map_err(|failed| format!("{key}: couldn't merge {onto}: {}", failed.why))?;
    Ok(MergeBegun {
        onto,
        onto_sha,
        before,
        files,
    })
}

/// What repository `key`'s branch of `feature` would take in, before any
/// merge: for an agent that only plans.
pub fn conflict_ends(feature: &Feature, key: &str) -> Result<MergeBegun, String> {
    let (_, onto, onto_sha, before) = merge_ends(feature, key)?;
    Ok(MergeBegun {
        onto,
        onto_sha,
        before,
        files: Vec::new(),
    })
}

/// How the conflicts an agent takes on stand (see `super::Resolving`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    /// The `Resolving` it was read for: its `at` and `planning`. A read of
    /// another is old by the time it is back.
    pub at: u64,
    pub planning: bool,
    /// A merge is under way in the worktree.
    pub merging: bool,
    /// The files still conflicted.
    pub left: Vec<String>,
    /// The branch's HEAD now.
    pub head: String,
    /// The base's commit is in the branch: the merge was committed.
    pub took_in: bool,
    /// The agent wrote its plan, or its questions.
    pub plan: bool,
    pub questions: bool,
}

/// Read how the conflicts `feature`'s agent takes on stand, its files in
/// data dir `data`; none when it takes none on.
pub fn read_resolution(feature: &Feature, data: &Path) -> Option<Resolution> {
    let resolving = feature.shipping.resolving.as_ref()?;
    let repo = feature.repo(&resolving.repo)?;
    let dir = &repo.worktree;
    let merging = git(dir, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok();
    let left = git(dir, &["diff", "--name-only", "--diff-filter=U"])
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let head = git(dir, &["rev-parse", "HEAD"]).map_or(String::new(), |h| h.trim().to_owned());
    let took_in = !resolving.onto_sha.is_empty()
        && git(
            dir,
            &["merge-base", "--is-ancestor", &resolving.onto_sha, "HEAD"],
        )
        .is_ok();
    Some(Resolution {
        at: resolving.at,
        planning: resolving.planning,
        merging,
        left,
        head,
        took_in,
        plan: resolving.plan(data).exists(),
        questions: resolving.questions(data).exists(),
    })
}

/// Stop the merge `feature`'s agent resolves: aborted in its worktree,
/// leaving the branch as it was.
pub fn abort_conflict_merge(feature: &Feature) -> Result<(), String> {
    let Some(resolving) = feature.shipping.resolving.as_ref() else {
        return Ok(());
    };
    let repo = (feature.repo(&resolving.repo))
        .ok_or_else(|| format!("{} has no repository {}", feature.title, resolving.repo))?;
    let dir = &repo.worktree;
    if git(dir, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_err() {
        // Committed already: it goes back to Verify, not as it was.
        let head = git(dir, &["rev-parse", "HEAD"]).map_or(String::new(), |h| h.trim().to_owned());
        let took_in = !resolving.onto_sha.is_empty()
            && git(
                dir,
                &["merge-base", "--is-ancestor", &resolving.onto_sha, "HEAD"],
            )
            .is_ok();
        if took_in && head != resolving.before && !resolving.planning {
            return Err(format!(
                "The merge in {} is committed already: it goes back to Verify",
                resolving.repo
            ));
        }
        return Ok(());
    }
    git_ops::abort_op(&repo.worktree, crate::git::InProgress::Merge)
        .map(|_| ())
        .map_err(|failed| {
            format!(
                "Couldn't stop the merge in {}: {}",
                resolving.repo, failed.why
            )
        })
}

/// What undoing a step that ran does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    /// Close `repo`'s pull request.
    ClosePr { repo: String, url: String },
    /// Delete `feature/<slug>` of `repo` on origin.
    DeleteBranch { repo: String, sha: Option<String> },
    /// Take the squash off `base` of `repo` in its main worktree, before it
    /// went to origin.
    ResetSquash { repo: String, base: String },
}

impl Undo {
    /// What follows, in words, for the dialog asking first: a line under
    /// its question, not the question again.
    pub fn label(&self, branch: &str) -> String {
        match self {
            Self::ClosePr { url, .. } => format!("Its pull request closes on GitHub: {url}"),
            Self::DeleteBranch { .. } => format!("{branch} goes off origin"),
            Self::ResetSquash { repo, base } => {
                format!("The squash comes off {base} in {repo}'s main worktree")
            }
        }
    }

    /// The repository it acts on, and the step it undoes.
    pub fn step(&self) -> (&str, ShipStep) {
        match self {
            Self::ClosePr { repo, .. } => (repo, ShipStep::OpenPr),
            Self::DeleteBranch { repo, .. } => (repo, ShipStep::Push),
            Self::ResetSquash { repo, .. } => (repo, ShipStep::Merge),
        }
    }
}

/// What undoing `step` of repository `repo` of `feature` does, in order, or
/// why it can't be undone. Undoing a push closes its pull request too: an
/// open one can't stay without its branch.
pub fn undo_of(feature: &Feature, repo: &str, step: ShipStep) -> Result<Vec<Undo>, String> {
    let ran = feature.ship_done(repo, step);
    if ran.is_none() {
        return Err(format!("{repo} has nothing of that to undo"));
    }
    let pr = || {
        (feature.ship_done(repo, ShipStep::OpenPr))
            .and_then(|d| d.url)
            .map(|url| Undo::ClosePr {
                repo: repo.to_owned(),
                url,
            })
    };
    let base = feature.repo(repo).map_or("its base", |r| r.base.as_str());
    match step {
        ShipStep::Push | ShipStep::PushBranch => Ok((pr().into_iter())
            .chain([Undo::DeleteBranch {
                repo: repo.to_owned(),
                sha: ran.and_then(|d| d.sha),
            }])
            .collect()),
        ShipStep::OpenPr => Ok(pr().into_iter().collect()),
        ShipStep::Merge if feature.ship_done(repo, ShipStep::PushBase).is_some() => Err(format!(
            "{base} of {repo} went to origin since: that can't be undone here"
        )),
        ShipStep::Merge => Ok(vec![Undo::ResetSquash {
            repo: repo.to_owned(),
            base: base.to_owned(),
        }]),
        ShipStep::PushBase => Err(format!(
            "{base} of {repo} is on origin: that can't be undone here"
        )),
    }
}

/// Run `undos` of `feature`, with `gh` the program to run as gh: the steps
/// undone, and why it stopped if it did.
pub fn run_undo(
    feature: &Feature,
    undos: &[Undo],
    gh: &str,
) -> (Vec<(String, ShipStep)>, Option<String>) {
    let branch = feature.branch();
    let mut undone = Vec::new();
    for undo in undos {
        let (key, step) = undo.step();
        let Some(repo) = feature.repo(key) else {
            return (
                undone,
                Some(format!("{} has no repository {key}", feature.title)),
            );
        };
        let result = match undo {
            Undo::ClosePr { url, .. } => close_pr(repo, url, gh),
            Undo::DeleteBranch { sha, .. } => delete_branch(repo, &branch, sha.as_deref()),
            Undo::ResetSquash { base, .. } => reset_squash(repo, base, &branch),
        };
        match result {
            Ok(()) => undone.push((key.to_owned(), step)),
            Err(why) => return (undone, Some(why)),
        }
    }
    (undone, None)
}

/// Close the pull request at `url` of `repo`: one closed already is fine, a
/// merged one can't be.
fn close_pr(repo: &Repo, url: &str, gh: &str) -> Result<(), String> {
    match run_gh(gh, &repo.worktree, &["pr", "close", url]) {
        Ok(_) => Ok(()),
        Err(why) if why.contains("already closed") => Ok(()),
        Err(why) if why.contains("merged") => Err(format!(
            "the pull request of {} is merged: that can't be undone here",
            repo.key()
        )),
        Err(why) => Err(format!(
            "Couldn't close the pull request of {}: {why}",
            repo.key()
        )),
    }
}

/// Delete `branch` of `repo` on origin: one gone already is fine.
fn delete_branch(repo: &Repo, branch: &str, sha: Option<&str>) -> Result<(), String> {
    let dir = if repo.worktree.is_dir() {
        &repo.worktree
    } else {
        &repo.main
    };
    match git_ops::delete_remote_branch(dir, "origin", branch, sha).map_err(|f| push_why(&f)) {
        Ok(_) => Ok(()),
        Err(why) if why.contains("remote ref does not exist") => Ok(()),
        Err(why) if why.contains("stale info") => Err(format!(
            "{branch} of {} moved on origin since it was pushed from here: left as it is",
            repo.key()
        )),
        Err(why) => Err(format!(
            "Couldn't delete {branch} of {} on origin: {why}",
            repo.key()
        )),
    }
}

/// Take the squash of `branch` off `base` in `repo`'s main worktree: only
/// while it is the last commit there, on `base`, not on origin, the
/// worktree clean.
fn reset_squash(repo: &Repo, base: &str, branch: &str) -> Result<(), String> {
    let name = repo.key();
    let main = &repo.main;
    let on = git(main, &["symbolic-ref", "-q", "--short", "HEAD"]).unwrap_or_default();
    if on.trim() != base {
        return Err(format!(
            "{name}'s main worktree isn't on {base}: switch it back to undo"
        ));
    }
    let status = git(main, &["status", "--porcelain", "--untracked-files=no"])?;
    if !status.trim().is_empty() {
        return Err(format!("{name}'s main worktree has uncommitted changes"));
    }
    // The last commit is the squash: it changes what the branch does.
    let not_squash = || format!("{base}'s last commit in {name} isn't the feature's squash");
    if git(main, &["rev-parse", "--verify", "-q", "HEAD~1"]).is_err() {
        return Err(not_squash());
    }
    let fork = git(main, &["merge-base", "HEAD~1", branch])?;
    let squash = patch::diff_id(main, "HEAD~1", "HEAD")?;
    let feature = patch::diff_id(main, fork.trim(), branch)?;
    if squash.is_empty() || squash != feature {
        return Err(not_squash());
    }
    // Origin as it is now: a push from another clone counts too.
    if let Err(failed) = git_ops::fetch_branch(main, "origin", base) {
        return Err(format!(
            "couldn't ask origin whether {base} of {name} has the squash: {}",
            failed.why
        ));
    }
    let remote = git(main, &["branch", "-r", "--contains", "HEAD"])?;
    if !remote.trim().is_empty() {
        return Err(format!(
            "{base} of {name} is on origin with the squash: that can't be undone here"
        ));
    }
    git(main, &["reset", "-q", "--keep", "HEAD~1"]).map(|_| ())
}

/// The repository `action` acts on, or "" for one on all.
pub(crate) fn action_repo(action: &Action) -> &str {
    match action {
        Action::Push { repo }
        | Action::OpenPr { repo, .. }
        | Action::KeepPr { repo, .. }
        | Action::Squash { repo, .. }
        | Action::PushBase { repo, .. } => repo,
        Action::LinkPrs => "",
    }
}

/// The body a feature's pull request starts with: its summary, if written,
/// then what feature it is part of.
fn pr_body(feature: &Feature) -> String {
    let part = format!("Part of “{}”.", feature.title);
    match summary_text(feature) {
        Some(summary) => format!("{summary}\n\n{part}"),
        None => part,
    }
}

/// `feature`'s summary (`SUMMARY_FILE`), trimmed, if one is written.
pub fn summary_text(feature: &Feature) -> Option<String> {
    let data = super::store::dir(None, &feature.project, &feature.slug);
    let (text, at) = page::read_summary(&data)?;
    // Asked again since: the old one, or half the new one, isn't it.
    if (feature.shipping.summary.as_ref()).is_some_and(|run| at < run.at) {
        return None;
    }
    let text = text.ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Push `branch` of `repo` to origin with its upstream, allowed for its
/// HEAD alone; guarded again after. With `lease`, it replaces what origin
/// has, as long as that is what was last fetched: after a rebase. The commit
/// pushed.
fn push(repo: &Repo, branch: &str, lease: bool) -> Result<String, String> {
    let head = git(&repo.worktree, &["rev-parse", "HEAD"])?;
    let head = head.trim().to_owned();
    hook::allow(&repo.main, branch, &head)?;
    let pushed = git_ops::push(&repo.worktree, Some("origin"), lease);
    let guarded = hook::guard(&repo.main, branch);
    if let Err(err) = &guarded {
        tracing::warn!("guarding {branch} of {} again: {err}", repo.key());
    }
    pushed.map(|_| head).map_err(|failed| push_why(&failed))
}

/// One line on why a push failed: git's own last line only says "failed
/// to push some refs" when a hook refused it, so the first line of what
/// was printed then.
fn push_why(failed: &git_ops::Failed) -> String {
    if !failed.why.starts_with("failed to push some refs") {
        return failed.why.clone();
    }
    let output = failed.log.last().map(|r| r.output.as_str()).unwrap_or("");
    output
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty()
                && !l.starts_with("To ")
                && !l.starts_with("error: failed to push")
                && !l.starts_with("hint:")
        })
        .unwrap_or(&failed.why)
        .to_owned()
}

/// The open pull request of `branch` of `repo`, or a new one into `base`:
/// its address.
fn open_pr(
    feature: &Feature,
    repo: &Repo,
    base: &str,
    branch: &str,
    gh: &str,
) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct View {
        url: String,
        state: String,
    }
    // An error here is mostly "no pull requests found": creating one says
    // what is wrong otherwise.
    if let Ok(out) = run_gh(
        gh,
        &repo.worktree,
        &["pr", "view", branch, "--json", "url,state"],
    ) && let Ok(view) = serde_json::from_str::<View>(&out)
        && view.state == "OPEN"
    {
        return Ok(view.url);
    }
    let body = pr_body(feature);
    let args = [
        "pr",
        "create",
        "--base",
        base,
        "--head",
        branch,
        "--title",
        feature.title.as_str(),
        "--body",
        body.as_str(),
    ];
    let out = run_gh(gh, &repo.worktree, &args)?;
    out.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "gh pr create printed no address".to_owned())
}

/// How long a gh command may take.
const GH_TIMEOUT: Duration = Duration::from_secs(120);

/// What `gh args`, run in `dir`, prints, or one line on why it failed.
pub(crate) fn run_gh(gh: &str, dir: &Path, args: &[&str]) -> Result<String, String> {
    let what = args.get(..2).unwrap_or(args).join(" ");
    let mut command = Command::new(gh);
    // The real gh, not a test's.
    if gh == "gh" {
        crate::ci::signed_in(&mut command)?;
    }
    command
        .args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1");
    let out = crate::t3_update::output_within(command, GH_TIMEOUT).map_err(|err| match err {
        missing if missing.starts_with("gh ") => missing,
        err => format!("gh {what}: {err}"),
    })?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let why = stderr.lines().map(str::trim).find(|l| !l.is_empty());
    Err(why.map_or_else(|| format!("gh {what} failed"), str::to_owned))
}

/// Push the base of `repo` to origin from its main worktree, which a squash
/// left on it, unless the user went elsewhere since: then that is what would
/// go. The commit pushed, or what stops it, in a line.
pub fn push_base(repo: &Repo) -> Result<String, String> {
    let on = git(&repo.main, &["symbolic-ref", "-q", "--short", "HEAD"])
        .map(|b| b.trim().to_owned())
        .unwrap_or_default();
    if on != repo.base {
        return Err(format!("its main worktree isn't on {} now", repo.base));
    }
    let common = git(
        &repo.main,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let _turn = git_ops::Turn::take(Path::new(common.trim()), "the push");
    let head = git(&repo.main, &["rev-parse", "HEAD"])?.trim().to_owned();
    git_ops::push(&repo.main, Some("origin"), false)
        .map(|_| head)
        .map_err(|failed| push_why(&failed))
}

/// The folder and the shell command of a squash ship of `feature`'s
/// `actions`, for a tab: every `Squash` action's repository in turn.
pub fn squash_command(feature: &Feature, actions: &[Action]) -> (PathBuf, String) {
    let branch = feature.branch();
    let mains: Vec<&Path> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Squash { repo, .. } => feature.repo(repo),
            _ => None,
        })
        .map(|r| r.main.as_path())
        .collect();
    let Some(first) = mains.first() else {
        return (feature.root.clone(), String::new());
    };
    // The summary, if written, is the message's body, to edit with the rest.
    let quote = crate::desktop::shell_quote;
    let body = summary_text(feature).map_or(String::new(), |text| format!(" -m {}", quote(&text)));
    let command = mains
        .iter()
        .map(|main| {
            format!(
                "cd {} && git merge --squash {} && git commit -e -m {}{body}",
                quote(&main.to_string_lossy()),
                quote(&branch),
                quote(&feature.title)
            )
        })
        .collect::<Vec<_>>()
        .join(" && ");
    (first.to_path_buf(), command)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::feature::UpdateWay;
    use crate::feature::tests::{Temp, commit, feature_over, link_over, run, try_run};

    /// Give each of `f`'s repositories a bare origin with main on it.
    fn origins(t: &Temp, f: &Feature) {
        for repo in &f.repos {
            let origin = t.0.join(format!("{}.git", repo.key().replace(':', "-")));
            std::fs::create_dir_all(&origin).unwrap();
            run(&origin, &["init", "-q", "--bare", "-b", "main"]);
            run(
                &repo.main,
                &["remote", "add", "origin", origin.to_str().unwrap()],
            );
            run(&repo.main, &["push", "-q", "origin", "main"]);
        }
    }

    /// What `f`'s ticks ask, by a fresh probe.
    fn planned(f: &Feature, modes: &Modes) -> Planned {
        plan_actions(f, &probe(f, modes).ship.unwrap())
    }

    /// Mark every stage before Ship on `f`'s code as it is now.
    fn mark_all(f: &mut Feature) {
        let patch = patch::read(f).unwrap().patch;
        while f.current() != Some(Stage::Ship) {
            f.tick(1, "you", Some(&patch));
        }
    }

    fn wt(f: &Feature, name: &str) -> PathBuf {
        f.repos
            .iter()
            .find(|r| r.name == name)
            .unwrap()
            .worktree
            .clone()
    }

    /// A fake gh in `t` logging its arguments to `gh.log`: `pr view`
    /// finds a pull request only once `open` is there, `pr create`
    /// prints an address naming the folder it ran in, or fails there when
    /// `refuse` names that folder.
    fn fake_gh(t: &Temp) -> String {
        let gh = t.0.join("gh");
        let script = format!(
            r#"#!/bin/sh
dir={dir}
printf '%s|' "$@" >> "$dir/gh.log"
echo >> "$dir/gh.log"
case "$1 $2" in
"pr view")
	if [ -e "$dir/open" ]; then
		echo '{{"url":"https://github.com/o/r/pull/9","state":"OPEN"}}'
	else
		echo "no pull requests found for branch \"$3\"" >&2
		exit 1
	fi ;;
"pr create")
	[ "$(cat "$dir/refuse" 2>/dev/null)" = "$(basename "$PWD")" ] && {{ echo "GraphQL: refused" >&2; echo more >&2; exit 1; }}
	echo "Creating pull request"
	echo "https://github.com/o/$(basename "$PWD")/pull/1" ;;
esac
"#,
            dir = t.0.display()
        );
        std::fs::write(&gh, script).unwrap();
        std::fs::set_permissions(&gh, PermissionsExt::from_mode(0o755)).unwrap();
        gh.to_string_lossy().into_owned()
    }

    fn gh_log(t: &Temp) -> String {
        std::fs::read_to_string(t.0.join("gh.log")).unwrap_or_default()
    }

    #[test]
    fn one_repository_ships_as_a_pull_request_and_again_keeps_it() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "login", "log in");
        mark_all(&mut f);
        let gh = fake_gh(&t);

        let p = probe(&f, &Modes::all(Some(ShipMode::Pr)));
        assert_eq!(p.guards["api"], Guard::On);
        assert!(hook::guarded(&f.repos[0].main, "feature/f"));
        let check = plan_actions(&f, &p.ship.unwrap());
        assert!(check.blockers.is_empty(), "{:?}", check.blockers);
        assert_eq!(
            check.actions,
            [
                Action::Push { repo: "api".into() },
                Action::OpenPr {
                    repo: "api".into(),
                    base: "main".into()
                }
            ]
        );
        // Guarded: a push by hand is refused.
        let err = try_run(&wt(&f, "api"), &["push", "-q", "origin", "HEAD"]).unwrap_err();
        assert!(err.contains("gerdr"), "{err}");

        // The hook that was there refuses it: its words, guarded after.
        let chained = f.repos[0].main.join(".git/hooks").join(hook::CHAINED);
        std::fs::write(&chained, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&chained, PermissionsExt::from_mode(0o755)).unwrap();
        let (why, so_far) = open_prs(&f, &check.actions, &gh).unwrap_err();
        assert_eq!(why, "Couldn't push feature/f of api: lint failed");
        assert!(so_far.is_empty());
        let guard_file = f.repos[0].main.join(".git/gerdr/guard/feature/f");
        assert_eq!(std::fs::read_to_string(&guard_file).unwrap(), "");
        std::fs::remove_file(&chained).unwrap();

        let prs = open_prs(&f, &check.actions, &gh).unwrap();
        assert_eq!(
            prs,
            [Pr {
                repo: "api".into(),
                url: "https://github.com/o/api/pull/1".into()
            }]
        );
        let origin = t.0.join("api.git");
        assert_eq!(
            run(&origin, &["rev-parse", "feature/f"]),
            run(&wt(&f, "api"), &["rev-parse", "HEAD"])
        );
        // Guarded again after.
        assert_eq!(std::fs::read_to_string(&guard_file).unwrap(), "");
        let log = gh_log(&t);
        assert!(log.contains("pr|view|feature/f|--json|url,state|"), "{log}");
        assert!(
            log.contains(
                "pr|create|--base|main|--head|feature/f|--title|Log in|--body|Part of “Log in”.|"
            ),
            "{log}"
        );

        // Shipped again: the pull request is kept, the push updates it.
        f.shipping.prs = prs.clone();
        let check = planned(&f, &Modes::all(Some(ShipMode::Pr)));
        assert_eq!(
            check.actions[1],
            Action::KeepPr {
                repo: "api".into(),
                url: prs[0].url.clone()
            }
        );
        std::fs::remove_file(t.0.join("gh.log")).unwrap();
        assert_eq!(open_prs(&f, &check.actions, &gh).unwrap(), prs);
        assert_eq!(gh_log(&t), "");
    }

    #[test]
    fn two_repositories_link_their_pull_requests() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "docs", "web"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a1", "api change");
        commit(&wt(&f, "web"), "w1", "web change");
        mark_all(&mut f);
        let gh = fake_gh(&t);
        let modes = Modes::all(Some(ShipMode::Pr));
        assert_eq!(
            probe(&f, &modes).ship.unwrap().notes,
            ["docs has no commits: left out"]
        );
        let check = planned(&f, &modes);
        assert_eq!(check.actions.len(), 5);
        assert_eq!(check.actions[4], Action::LinkPrs);
        assert_eq!(
            check.actions[4].label("feature/x", false),
            "Link the pull requests to each other"
        );

        // web's create fails: api's is kept for the run again.
        std::fs::write(t.0.join("refuse"), "web").unwrap();
        let (why, so_far) = open_prs(&f, &check.actions, &gh).unwrap_err();
        assert_eq!(why, "Couldn't open a pull request of web: GraphQL: refused");
        assert_eq!(so_far.len(), 1);
        std::fs::remove_file(t.0.join("refuse")).unwrap();

        f.shipping.prs = so_far;
        let check = planned(&f, &Modes::all(Some(ShipMode::Pr)));
        assert!(matches!(check.actions[1], Action::KeepPr { .. }));
        let prs = open_prs(&f, &check.actions, &gh).unwrap();
        let urls: Vec<&str> = prs.iter().map(|p| p.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://github.com/o/api/pull/1",
                "https://github.com/o/web/pull/1"
            ]
        );
        let log = gh_log(&t);
        assert!(
            log.contains(
                "pr|edit|https://github.com/o/api/pull/1|--body|Part of “Log in”.\n\n\
                 The others:\n- web: https://github.com/o/web/pull/1\n|"
            ),
            "{log}"
        );
        assert!(
            log.contains("- api: https://github.com/o/api/pull/1\n|"),
            "{log}"
        );

        // One open for the branch already is used, not made again.
        std::fs::write(t.0.join("open"), "").unwrap();
        let fresh = planned(&feature_over_again(&f), &Modes::all(Some(ShipMode::Pr)));
        let prs = open_prs(&f, &fresh.actions, &gh).unwrap();
        assert_eq!(prs[0].url, "https://github.com/o/r/pull/9");
    }

    /// `f` with no pull requests kept.
    fn feature_over_again(f: &Feature) -> Feature {
        let mut f = f.clone();
        f.shipping.prs.clear();
        f
    }

    #[test]
    fn what_stops_shipping() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "docs"], &["docs"]);
        origins(&t, &f);
        mark_all(&mut f);
        // No commits: nothing to do, which stops nothing.
        let none = planned(&f, &Modes::all(Some(ShipMode::Pr)));
        assert!(none.nothing);
        assert!(none.blockers.is_empty());

        // The marks were given on code without commits.
        commit(&wt(&f, "api"), "a1", "change");
        std::fs::write(wt(&f, "api").join("a1"), "more").unwrap();
        std::fs::write(wt(&f, "docs").join("a"), "x").unwrap();
        let check = planned(&f, &Modes::all(Some(ShipMode::Pr)));
        assert_eq!(
            check.blockers,
            [
                "Code changed since Build was marked",
                "api has uncommitted changes",
                "docs has changes, but the feature doesn't change it",
            ]
        );
        assert!(check.actions.is_empty());

        // Not at Ship: no check.
        f.untick(Stage::Verify, 2);
        assert!(probe(&f, &Modes::default()).ship.is_none());
    }

    #[test]
    fn open_findings_stop_shipping() {
        use crate::feature::review_import::{AiReview, Finding, FindingState};
        let mut f = crate::features::tests::feature("a", vec![], 7);
        let finding = |state| Finding {
            state,
            ..Finding::default()
        };
        f.review = Some(AiReview {
            findings: vec![
                finding(FindingState::Open),
                finding(FindingState::Sent),
                finding(FindingState::Dismissed),
            ],
            ..AiReview::default()
        });
        let err = Err("gone".to_owned());
        let blockers = check(&f, &err, &Modes::default()).blockers;
        assert!(
            blockers.contains(&"2 AI review findings open".to_owned()),
            "{blockers:?}"
        );
        f.review.as_mut().unwrap().findings.truncate(1);
        let blockers = check(&f, &err, &Modes::default()).blockers;
        assert!(
            blockers.contains(&"1 AI review finding open".to_owned()),
            "{blockers:?}"
        );
        f.review = None;
        let blockers = check(&f, &err, &Modes::default()).blockers;
        assert!(
            !blockers.iter().any(|b| b.contains("AI review")),
            "{blockers:?}"
        );
    }

    #[test]
    fn a_squash_needs_its_main_worktree_on_the_base_and_clean() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let main = f.repos[0].main.clone();
        commit(&wt(&f, "api"), "a", "feature side");
        mark_all(&mut f);
        // No origin on GitHub: a squash.
        assert_eq!(mode_of(&f, "api", &Modes::default()), ShipMode::Squash);
        let check = |f: &Feature| planned(f, &Modes::default());
        assert_eq!(
            check(&f).actions,
            [Action::Squash {
                repo: "api".into(),
                base: "main".into()
            }]
        );

        std::fs::write(main.join("a"), "edited").unwrap();
        assert_eq!(
            check(&f).blockers,
            ["api's main worktree has uncommitted changes on main"]
        );
        run(&main, &["checkout", "-q", "--", "a"]);
        // Untracked files don't stop it.
        std::fs::write(main.join("loose"), "x").unwrap();
        assert!(check(&f).blockers.is_empty());

        run(&main, &["switch", "-q", "-c", "other"]);
        assert_eq!(
            check(&f).blockers,
            ["api's main worktree is on other: switch it to main"]
        );
        run(&main, &["switch", "-q", "main"]);

        commit(&main, "a", "main side");
        assert_eq!(
            check(&f).blockers,
            ["api: squashing into main conflicts in a"]
        );
    }

    #[test]
    fn a_squash_runs_in_a_tab_and_reads_as_merged_once_committed() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "web"], &[]);
        f.title = "Don't break".into();
        commit(&wt(&f, "api"), "x", "feature");
        mark_all(&mut f);
        let check = planned(&f, &Modes::default());
        let (dir, command) = squash_command(&f, &check.actions);
        let main = f.repos[0].main.clone();
        assert_eq!(dir, main);
        assert_eq!(
            command,
            format!(
                "cd {} && git merge --squash feature/f && git commit -e -m 'Don'\\''t break'",
                crate::desktop::shell_quote(&main.to_string_lossy())
            )
        );
        assert_eq!(squash_command(&f, &[]), (f.root.clone(), String::new()));

        f.shipping.squash_at = Some(5);
        assert!(!probe(&f, &Modes::default()).merged);
        run(&main, &["merge", "-q", "--squash", "feature/f"]);
        run(&main, &["commit", "-q", "-m", "Don't break"]);
        let p = probe(&f, &Modes::default());
        assert!(p.merged);
        f.shipping.squash_at = None;
        assert!(!probe(&f, &Modes::default()).merged);
    }

    #[test]
    fn each_project_ships_its_own_way_and_squashes_in_its_own_main() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        link_over(&t, &mut f, "other", &["api", "docs"], &["docs"]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "x", "home change");
        commit(&f.repos[1].worktree.clone(), "y", "linked change");
        mark_all(&mut f);
        let modes = Modes {
            home: Some(ShipMode::Pr),
            links: [("other".to_owned(), Some(ShipMode::Squash))].into(),
        };
        assert_eq!(mode_of(&f, "other:api", &modes), ShipMode::Squash);
        assert_eq!(mode_of(&f, "api", &modes), ShipMode::Pr);
        let p = probe(&f, &modes);
        assert_eq!(p.guards.keys().collect::<Vec<_>>(), ["api", "other:api"]);
        let check = plan_actions(&f, &p.ship.unwrap());
        assert!(check.blockers.is_empty(), "{:?}", check.blockers);
        assert_eq!(
            check.actions,
            [
                Action::Push { repo: "api".into() },
                Action::OpenPr {
                    repo: "api".into(),
                    base: "main".into()
                },
                Action::Squash {
                    repo: "other:api".into(),
                    base: "main".into()
                },
            ]
        );
        let (dir, command) = squash_command(&f, &check.actions);
        let linked = f.repos[1].main.clone();
        assert_eq!(dir, linked);
        let cd = crate::desktop::shell_quote(&linked.to_string_lossy());
        assert!(command.starts_with(&format!("cd {cd} &&")));

        let gh = fake_gh(&t);
        let prs = open_prs(&f, &check.actions, &gh).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].repo, "api");
    }

    /// Set `key`'s route on `f`, its ticks as they are.
    fn route(f: &mut Feature, key: &str, route: Route) {
        let plan = f.ship_plan.repos.entry(key.into());
        plan.or_insert_with(|| RepoPlan::new(route)).route = route;
    }

    #[test]
    fn the_route_picks_the_steps_whatever_the_project_sets() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a", "change");
        mark_all(&mut f);
        let modes = Modes::all(Some(ShipMode::Pr));
        let api = || "api".to_owned();

        route(&mut f, "api", Route::Merge);
        let merge = Action::Squash {
            repo: api(),
            base: "main".into(),
        };
        assert_eq!(planned(&f, &modes).actions, std::slice::from_ref(&merge));

        // Push the base too: run after the squash, named in the tab's stead.
        let plan = f.ship_plan.repos.get_mut("api").unwrap();
        plan.toggle(ShipStep::PushBase);
        let check = planned(&f, &modes);
        assert_eq!(
            check.actions,
            [
                merge.clone(),
                Action::PushBase {
                    repo: api(),
                    base: "main".into()
                }
            ]
        );
        let (_, command) = squash_command(&f, &check.actions);
        assert!(!command.contains("git push"), "{command}");

        // Push the feature's branch too: through origin, before the squash.
        let plan = f.ship_plan.repos.get_mut("api").unwrap();
        plan.toggle(ShipStep::PushBranch);
        let check = planned(&f, &modes);
        assert_eq!(
            check.actions,
            [
                Action::Push { repo: api() },
                merge,
                Action::PushBase {
                    repo: api(),
                    base: "main".into()
                }
            ]
        );
        let (_, command) = squash_command(&f, &check.actions);
        assert!(!command.contains("git push"), "{command}");

        // Push without a pull request.
        route(&mut f, "api", Route::Pr);
        let plan = f.ship_plan.repos.get_mut("api").unwrap();
        plan.toggle(ShipStep::OpenPr);
        assert_eq!(planned(&f, &modes).actions, [Action::Push { repo: api() }]);

        route(&mut f, "api", Route::Nothing);
        let none = planned(&f, &modes);
        assert!(none.nothing && none.actions.is_empty());
    }

    #[test]
    fn nothing_ticked_stops_nothing_and_a_ticked_step_that_cant_run_stops_it() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        commit(&wt(&f, "api"), "a", "change");
        mark_all(&mut f);
        // Uncommitted, and no origin: no pull request can go.
        std::fs::write(wt(&f, "api").join("a"), "more").unwrap();
        let modes = Modes::all(Some(ShipMode::Pr));
        let check = planned(&f, &modes);
        assert!(!check.nothing);
        assert_eq!(
            check.blockers,
            ["api has uncommitted changes", "api has no origin remote"]
        );
        assert!(check.actions.is_empty());

        let mut plan = RepoPlan::new(Route::Pr);
        plan.toggle(ShipStep::Push);
        f.ship_plan.repos.insert("api".into(), plan);
        let none = planned(&f, &modes);
        assert!(none.nothing);
        assert!(none.blockers.is_empty(), "{:?}", none.blockers);
    }

    #[test]
    fn a_step_not_ticked_doesnt_stop_it() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        let main = f.repos[0].main.clone();
        commit(&wt(&f, "api"), "a", "change");
        mark_all(&mut f);
        // The main worktree stops a merge, which isn't what is asked.
        run(&main, &["switch", "-q", "-c", "other"]);
        let modes = Modes::all(Some(ShipMode::Pr));
        assert!(planned(&f, &modes).blockers.is_empty());
        route(&mut f, "api", Route::Merge);
        assert_eq!(
            planned(&f, &modes).blockers,
            ["api's main worktree is on other: switch it to main"]
        );
        // The tick stays: it goes once the cause does.
        run(&main, &["switch", "-q", "main"]);
        assert!(planned(&f, &modes).blockers.is_empty());
    }

    #[test]
    fn only_those_squashing_hold_the_merge_open() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "web"], &[]);
        commit(&wt(&f, "api"), "a", "api change");
        commit(&wt(&f, "web"), "w", "web change");
        mark_all(&mut f);
        f.shipping.squash_at = Some(5);
        f.shipping.squash_repos = vec!["api".into()];
        let modes = Modes::default();
        assert!(!probe(&f, &modes).merged);
        let main = f.repos[0].main.clone();
        run(&main, &["merge", "-q", "--squash", "feature/f"]);
        run(&main, &["commit", "-q", "-m", "api"]);
        assert!(probe(&f, &modes).merged, "web stays out of it");
        // A record from before these were kept waits for all.
        f.shipping.squash_repos.clear();
        assert!(!probe(&f, &modes).merged);
    }

    #[test]
    fn a_branch_merged_already_has_nothing_left_to_run() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        commit(&wt(&f, "api"), "a", "change");
        mark_all(&mut f);
        let main = f.repos[0].main.clone();
        run(&main, &["merge", "-q", "--squash", "feature/f"]);
        run(&main, &["commit", "-q", "-m", "by hand"]);
        let check = probe(&f, &Modes::default()).ship.unwrap();
        assert!(check.repos[0].merged);
        assert!(check.repos[0].no_merge.is_none());
        assert!(check.notes.is_empty());
        let planned = plan_actions(&f, &check);
        assert!(planned.all_done && !planned.nothing);
        assert!(planned.actions.is_empty());
        assert!(removable(&f, &check), "nothing is lost");
    }

    /// What ran is kept, and a later ship leaves it out: a push stays done
    /// while the branch is where it went from.
    #[test]
    fn keeping_a_pull_request_counts_as_no_step() {
        let planned = Planned {
            actions: vec![
                Action::Push { repo: "api".into() },
                Action::KeepPr {
                    repo: "api".into(),
                    url: "https://github.com/o/api/pull/7".into(),
                },
                Action::LinkPrs,
            ],
            ..Planned::default()
        };
        assert_eq!(planned.steps(), 1);
    }

    #[test]
    fn what_ran_is_left_out_until_the_branch_moves() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a1", "api change");
        mark_all(&mut f);
        let gh = fake_gh(&t);
        let modes = Modes::all(Some(ShipMode::Pr));
        // Only the push ticked: the pull request comes later.
        let check = probe(&f, &modes).ship.unwrap();
        f.ship_plan
            .repos
            .insert("api".into(), RepoPlan::new(Route::Pr));
        f.ship_plan
            .repos
            .get_mut("api")
            .unwrap()
            .toggle(ShipStep::OpenPr);
        let planned = plan_actions(&f, &check);
        assert_eq!(planned.actions, [Action::Push { repo: "api".into() }]);
        let (ran, failed) = run_steps(&f, &planned.actions, &gh, 7);
        assert_eq!(failed, None);
        let head = run(&wt(&f, "api"), &["rev-parse", "HEAD"]);
        assert_eq!(ran.done.len(), 1);
        assert_eq!(ran.done[0].sha.as_deref(), Some(head.trim()));
        assert_eq!(ran.done[0].at, 7);
        for done in ran.done {
            f.ship_ran(done);
        }
        f.tick(8, "gerdr", None);
        assert!(f.ships_more());

        // Shipped, the push ran: nothing is left until more is ticked.
        let check = probe(&f, &modes)
            .ship
            .expect("a shipped feature is checked");
        let planned = plan_actions(&f, &check);
        assert!(planned.all_done, "{planned:?}");
        f.ship_plan
            .repos
            .get_mut("api")
            .unwrap()
            .toggle(ShipStep::OpenPr);
        let planned = plan_actions(&f, &check);
        assert_eq!(
            planned.actions,
            [Action::OpenPr {
                repo: "api".into(),
                base: "main".into()
            }]
        );

        // The branch moved: the push is to run again, once the code is
        // gone over again.
        commit(&wt(&f, "api"), "a2", "more");
        let check = probe(&f, &modes).ship.unwrap();
        assert!(step_done(&f, &check.repos[0], ShipStep::Push).is_none());
        assert!(step_done(&f, &check.repos[0], ShipStep::PushBranch).is_none());
        let planned = plan_actions(&f, &check);
        assert!(
            (planned.blockers.iter()).any(|b| b.starts_with("Code changed since")),
            "{planned:?}"
        );
    }

    /// A squash not on origin comes off its base, a pushed branch off
    /// origin; a base pushed stays.
    #[test]
    fn undo_takes_back_what_isnt_on_origins_base() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a1", "api change");
        mark_all(&mut f);
        let gh = fake_gh(&t);
        let (ran, failed) = run_steps(&f, &[Action::Push { repo: "api".into() }], &gh, 5);
        assert_eq!(failed, None);
        for done in ran.done {
            f.ship_ran(done);
        }
        let main = f.repos[0].main.clone();
        let before = run(&main, &["rev-parse", "HEAD"]);
        run(&main, &["merge", "-q", "--squash", "feature/f"]);
        run(&main, &["commit", "-q", "-m", "squashed"]);
        f.ship_ran(ShipDone {
            at: 6,
            repo: "api".into(),
            step: ShipStep::Merge,
            sha: None,
            url: None,
            undone: None,
        });

        assert!(undo_of(&f, "api", ShipStep::PushBase).is_err(), "never ran");
        let undos = undo_of(&f, "api", ShipStep::Merge).unwrap();
        assert_eq!(
            undos,
            [Undo::ResetSquash {
                repo: "api".into(),
                base: "main".into()
            }]
        );
        let (undone, failed) = run_undo(&f, &undos, &gh);
        assert_eq!(failed, None);
        assert_eq!(undone, [("api".to_owned(), ShipStep::Merge)]);
        assert_eq!(run(&main, &["rev-parse", "HEAD"]), before);
        // Gone already, it isn't taken off again.
        let (_, failed) = run_undo(&f, &undos, &gh);
        assert_eq!(
            failed.as_deref(),
            Some("main's last commit in api isn't the feature's squash")
        );

        let undos = undo_of(&f, "api", ShipStep::PushBranch).unwrap();
        let pushed = f.ship_done("api", ShipStep::Push).and_then(|d| d.sha);
        assert!(pushed.is_some());
        assert_eq!(
            undos,
            [Undo::DeleteBranch {
                repo: "api".into(),
                sha: pushed
            }]
        );
        let (_, failed) = run_undo(&f, &undos, &gh);
        assert_eq!(failed, None);
        let origin = t.0.join("api.git");
        assert!(try_run(&origin, &["rev-parse", "--verify", "-q", "feature/f"]).is_err());
        // Gone already: nothing to undo, nothing fails.
        let (_, failed) = run_undo(&f, &undos, &gh);
        assert_eq!(failed, None);
        f.ship_undid("api", ShipStep::Push, 9);
        assert!(f.ship_done("api", ShipStep::PushBranch).is_none());
        assert!(
            f.shipping
                .steps
                .iter()
                .all(|d| d.step != ShipStep::Push || d.undone == Some(9))
        );

        // A base on origin stays.
        f.ship_ran(ShipDone {
            at: 10,
            repo: "api".into(),
            step: ShipStep::PushBase,
            sha: None,
            url: None,
            undone: None,
        });
        assert!(undo_of(&f, "api", ShipStep::PushBase).is_err());
    }

    /// A base that moved on reads as behind, and as conflicting where it
    /// does; taking it in merges it, or leaves all as it was.
    #[test]
    fn a_base_that_moved_on_is_read_and_taken_in() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a1", "api change");
        mark_all(&mut f);
        let main = f.repos[0].main.clone();
        let bases = read_bases(&f, false);
        assert_eq!(bases["api"].behind, 0);
        assert!(!bases["api"].clean());

        // The base moves on, elsewhere: fetched, it is behind, cleanly.
        commit(&main, "m1", "on main");
        run(&main, &["push", "-q", "origin", "main"]);
        let bases = read_bases(&f, true);
        assert_eq!(bases["api"].onto, "origin/main");
        assert_eq!(bases["api"].behind, 1);
        assert!(bases["api"].clean(), "{:?}", bases["api"]);
        assert!(!bases["api"].merges);

        let (update, patch) = update_from_base(&f, "api", UpdateWay::Merge, 7).unwrap();
        assert_eq!(update.onto, "origin/main");
        assert_ne!(update.before, update.after);
        assert!(patch.is_some());
        let bases = read_bases(&f, false);
        assert_eq!(bases["api"].behind, 0);
        assert!(bases["api"].merges, "the merge is in its history now");
        // A local main that lacks what was taken in: squashing waits for a
        // pull, as the squash would take it in as the feature's own.
        assert_eq!(squash_blocker(&f.repos[0], &f.branch()), None);
        run(&main, &["reset", "-q", "--hard", "HEAD~1"]);
        assert_eq!(
            squash_blocker(&f.repos[0], &f.branch()).as_deref(),
            Some("api: feature/f has a commit of origin/main that main lacks: pull main first")
        );
        let check = |no_merge: Option<String>| RepoCheck {
            key: "api".into(),
            base: "main".into(),
            main: main.clone(),
            commits: 1,
            route: Route::Merge,
            no_origin: None,
            no_merge,
            merged: false,
            head: None,
            pushed: None,
        };
        assert!(check(squash_blocker(&f.repos[0], &f.branch())).pulls_base());
        assert_eq!(pull_base(&f.repos[0]), Ok(1));
        assert_eq!(squash_blocker(&f.repos[0], &f.branch()), None);
        assert!(!check(Some("api: squashing into main conflicts in c".into())).pulls_base());

        // A change to the same file conflicts: read so, and refused as it was.
        commit(&wt(&f, "api"), "c", "feature side");
        commit(&main, "c", "base side");
        run(&main, &["push", "-q", "origin", "main"]);
        let bases = read_bases(&f, true);
        assert_eq!(bases["api"].conflicts, ["c"]);
        let head = run(&wt(&f, "api"), &["rev-parse", "HEAD"]);
        let why = update_from_base(&f, "api", UpdateWay::Merge, 8).unwrap_err();
        assert!(
            why.starts_with("api: couldn't take origin/main in"),
            "{why}"
        );
        assert_eq!(run(&wt(&f, "api"), &["rev-parse", "HEAD"]), head);
        let status = run(&wt(&f, "api"), &["status", "--porcelain"]);
        assert!(status.trim().is_empty(), "{status}");
    }

    /// A merge for an agent starts in the worktree and stops on its
    /// conflicts; read, it is under way, and stopping it leaves all as it was.
    #[test]
    fn a_merge_for_an_agent_starts_reads_and_stops() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "c", "feature side");
        mark_all(&mut f);
        let main = f.repos[0].main.clone();
        commit(&main, "c", "base side");
        run(&main, &["push", "-q", "origin", "main"]);
        read_bases(&f, true);
        let head = run(&wt(&f, "api"), &["rev-parse", "HEAD"]);
        let begun = begin_conflict_merge(&f, "api").unwrap();
        assert_eq!(begun.files, ["c"]);
        assert_eq!(begun.onto, "origin/main");
        assert_eq!(begun.before, head.trim());
        f.shipping.resolving = Some(super::super::Resolving {
            at: 1,
            repo: "api".into(),
            onto: begun.onto.clone(),
            onto_sha: begun.onto_sha.clone(),
            before: begun.before.clone(),
            files: begun.files.clone(),
            agent: "claude".into(),
            pane: None,
            n: 1,
            planning: false,
            nudged: false,
        });
        let data = t.0.join("data");
        let found = read_resolution(&f, &data).unwrap();
        assert!(found.merging && !found.took_in);
        assert_eq!(found.left, ["c"]);
        abort_conflict_merge(&f).unwrap();
        let found = read_resolution(&f, &data).unwrap();
        assert!(!found.merging && !found.took_in);
        assert_eq!(run(&wt(&f, "api"), &["rev-parse", "HEAD"]), head);

        // Resolved and committed: the base is in.
        begin_conflict_merge(&f, "api").unwrap();
        std::fs::write(wt(&f, "api").join("c"), "both sides").unwrap();
        run(&wt(&f, "api"), &["add", "c"]);
        run(&wt(&f, "api"), &["commit", "-q", "--no-edit"]);
        let found = read_resolution(&f, &data).unwrap();
        assert!(!found.merging && found.took_in && found.left.is_empty());
        assert_ne!(found.head, head.trim());
    }

    /// Updated from its base, a shipped feature is open at Verify again,
    /// Build's mark on the code as it is now.
    #[test]
    fn an_update_goes_back_to_verify() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        commit(&wt(&f, "api"), "a1", "api change");
        mark_all(&mut f);
        f.tick(5, "gerdr", None);
        assert_eq!(f.status, Status::Done);
        let update = super::super::BaseUpdate {
            at: 6,
            repo: "api".into(),
            way: UpdateWay::Rebase,
            onto: "origin/main".into(),
            onto_sha: "abc".into(),
            before: "1".into(),
            after: "2".into(),
        };
        f.updated_from_base(update, Some("new-patch"), 6);
        assert_eq!(f.status, Status::Open);
        assert_eq!(f.current(), Some(Stage::Verify));
        assert_eq!(f.back_from, Some(Stage::Ship));
        let build = f.marks.get(&Stage::Build).unwrap();
        assert_eq!(build.patch.as_deref(), Some("new-patch"));
        assert_eq!(f.stale("new-patch"), None);
        assert!(f.rebased_since_push("api"), "a rebase pushes with lease");
    }

    /// Origin's base taken in while the local base stays behind isn't the
    /// feature's change: not in its diff, line count or patch-id, and
    /// pulling the local base after changes none of them.
    #[test]
    fn a_base_taken_in_is_not_the_features_change() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a1", "api change");
        let own = patch::read(&f).unwrap().patch;
        // Origin's main moves on by two commits; the local main stays behind.
        let main = f.repos[0].main.clone();
        commit(&main, "m1", "on main");
        commit(&main, "m2", "more on main");
        run(&main, &["push", "-q", "origin", "main"]);
        run(&main, &["reset", "-q", "--hard", "HEAD~2"]);

        let (update, patch) = update_from_base(&f, "api", UpdateWay::Merge, 7).unwrap();
        assert_eq!(patch.as_deref(), Some(own.as_str()));
        f.updated_from_base(update.clone(), patch.as_deref(), 7);
        assert_eq!(f.repos[0].base_commit, update.onto_sha);
        let reading = patch::read(&f).unwrap();
        assert_eq!(reading.patch, own);
        assert_eq!(reading.commits["api"], 2, "a1 and the merge");
        let stat = super::super::page::stat(&f.repos[0]).unwrap();
        assert_eq!((stat.files, stat.added), (1, 1));
        let read = super::super::viewed::read(&f.repos[0], &Default::default()).unwrap();
        let paths: Vec<_> = read.blobs.keys().collect();
        assert_eq!(paths, ["a1"]);

        // Pulling the local main changes nothing of the feature's.
        run(&main, &["merge", "-q", "--ff-only", "origin/main"]);
        let pulled = patch::read(&f).unwrap();
        assert_eq!(pulled.patch, own);
        assert_eq!(pulled.commits, reading.commits);
        let stat = super::super::page::stat(&f.repos[0]).unwrap();
        assert_eq!((stat.files, stat.added), (1, 1));
    }

    /// A push from before the log kept its commit holds only while origin's
    /// branch is where the worktree is.
    #[test]
    fn an_old_push_holds_only_while_origin_has_the_head() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        f.shipping.pushed = vec!["api".into()];
        let repo = |head: &str, pushed: Option<&str>| RepoCheck {
            key: "api".into(),
            base: "main".into(),
            main: PathBuf::from("/m"),
            commits: 1,
            route: Route::Pr,
            no_origin: None,
            no_merge: None,
            merged: false,
            head: Some(head.into()),
            pushed: pushed.map(str::to_owned),
        };
        assert!(step_done(&f, &repo("a", Some("a")), ShipStep::Push).is_some());
        assert!(step_done(&f, &repo("b", Some("a")), ShipStep::Push).is_none());
        assert!(step_done(&f, &repo("a", None), ShipStep::Push).is_none());
        f.shipping.steps.clear();
        f.ship_ran(ShipDone {
            at: 3,
            repo: "api".into(),
            step: ShipStep::Push,
            sha: Some("c".into()),
            url: None,
            undone: None,
        });
        assert!(step_done(&f, &repo("c", None), ShipStep::Push).is_some());
    }

    /// A record from before the log was kept reads as one.
    #[test]
    fn an_old_record_reads_as_a_log() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "web"], &[]);
        f.shipping.pushed = vec!["api".into()];
        f.shipping.prs = vec![Pr {
            repo: "api".into(),
            url: "https://github.com/o/api/pull/3".into(),
        }];
        f.shipping.squash_at = Some(5);
        f.shipping.squash_repos = vec!["web".into()];
        f.shipping.push_base = vec!["web".into()];
        // Not merged yet: the squash isn't in it.
        let steps: Vec<ShipStep> = f.ship_log().iter().map(|d| d.step).collect();
        assert_eq!(steps, [ShipStep::Push, ShipStep::OpenPr]);
        assert!(f.squash_going());
        while f.current().is_some() {
            f.tick(9, "gerdr", None);
        }
        let steps: Vec<ShipStep> = f.ship_log().iter().map(|d| d.step).collect();
        assert_eq!(
            steps,
            [
                ShipStep::Push,
                ShipStep::OpenPr,
                ShipStep::Merge,
                ShipStep::PushBase
            ]
        );
        assert!(f.ship_log().iter().all(|d| d.at == 9));
        assert!(!f.squash_going(), "merged when Ship was done");
        assert!(f.ship_done("api", ShipStep::PushBranch).is_some());
        // A step that runs now keeps those before it.
        f.ship_ran(ShipDone {
            at: 10,
            repo: "web".into(),
            step: ShipStep::Push,
            sha: None,
            url: None,
            undone: None,
        });
        assert_eq!(f.shipping.steps.len(), 5);
    }

    #[test]
    fn a_squashed_branch_loses_nothing_and_goes_with_its_worktree() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        commit(&wt(&f, "api"), "a", "change");
        mark_all(&mut f);
        let main = f.repos[0].main.clone();
        assert_eq!(
            crate::feature::layout::losses(&f),
            ["api: 1 commit not in main"]
        );
        run(&main, &["merge", "-q", "--squash", "feature/f"]);
        run(&main, &["commit", "-q", "-m", "squashed"]);
        assert!(crate::feature::layout::losses(&f).is_empty());
        crate::feature::layout::remove(&f, false).unwrap();
        assert!(!wt(&f, "api").exists());
        assert!(try_run(&main, &["rev-parse", "--verify", "-q", "feature/f"]).is_err());
    }

    #[test]
    fn a_branch_merged_on_origin_loses_nothing_before_main_is_pulled() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a", "change");
        mark_all(&mut f);
        let main = f.repos[0].main.clone();
        // Merged on GitHub: squashed into origin's main from elsewhere.
        let other = t.0.join("other");
        let origin = t.0.join("api.git");
        let (origin, other_dir) = (origin.to_str().unwrap(), other.to_str().unwrap());
        run(&t.0, &["clone", "-q", origin, other_dir]);
        run(&main, &["push", "-q", "origin", "feature/f"]);
        run(&other, &["fetch", "-q", "origin", "feature/f"]);
        run(&other, &["merge", "-q", "--squash", "FETCH_HEAD"]);
        let who = ["-c", "user.name=t", "-c", "user.email=t@t"];
        run(
            &other,
            &[&who[..], &["commit", "-q", "-m", "squashed"]].concat(),
        );
        run(&other, &["push", "-q", "origin", "main"]);
        assert!(crate::feature::layout::losses(&f).is_empty());
    }

    #[test]
    fn a_branch_fast_forwarded_on_origin_loses_nothing_and_still_counts() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        commit(&wt(&f, "api"), "a", "change");
        mark_all(&mut f);
        let main = f.repos[0].main.clone();
        // Merged on GitHub as a fast-forward, from elsewhere: origin's main
        // is its tip, and here origin's main is as it was.
        let other = t.0.join("other");
        let origin = t.0.join("api.git");
        let (origin, other_dir) = (origin.to_str().unwrap(), other.to_str().unwrap());
        run(&t.0, &["clone", "-q", origin, other_dir]);
        run(&main, &["push", "-q", "origin", "feature/f"]);
        run(&other, &["fetch", "-q", "origin", "feature/f"]);
        let to_main = ["push", "-q", "origin", "FETCH_HEAD:refs/heads/main"];
        run(&other, &to_main);
        assert!(crate::feature::layout::losses(&f).is_empty());
        // Fetched now, origin's main has its tip: what it shipped still
        // counts, for the recap, not an empty change.
        let reading = super::patch::read(&f).unwrap();
        assert_eq!(reading.commits[&f.repos[0].key()], 1);
        assert_eq!(super::page::stat(&f.repos[0]).unwrap().added, 1);
    }

    #[test]
    fn removing_goes_only_with_every_repository_merging() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "web"], &[]);
        commit(&wt(&f, "api"), "a", "api change");
        commit(&wt(&f, "web"), "w", "web change");
        mark_all(&mut f);
        let modes = Modes::all(Some(ShipMode::Squash));
        let check = probe(&f, &modes).ship.unwrap();
        assert!(removable(&f, &check));
        route(&mut f, "web", Route::Nothing);
        assert!(!removable(&f, &check));
        route(&mut f, "web", Route::Merge);
        assert!(removable(&f, &check));
        f.ship_plan
            .repos
            .get_mut("web")
            .unwrap()
            .toggle(ShipStep::Merge);
        assert!(!removable(&f, &check));
    }

    #[test]
    fn pushing_the_base_goes_from_its_branch_only() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        origins(&t, &f);
        let repo = &f.repos[0];
        commit(&repo.main, "m", "on main");
        push_base(repo).unwrap();
        let origin = t.0.join("api.git");
        assert_eq!(
            run(&origin, &["rev-parse", "main"]),
            run(&repo.main, &["rev-parse", "main"])
        );
        run(&repo.main, &["switch", "-q", "-c", "other"]);
        assert_eq!(
            push_base(repo).unwrap_err(),
            "its main worktree isn't on main now"
        );
    }

    #[test]
    fn a_tick_keeps_what_it_needs() {
        let mut plan = RepoPlan::new(Route::Pr);
        plan.toggle(ShipStep::Push);
        assert!(!plan.ticked(ShipStep::OpenPr));
        plan.toggle(ShipStep::OpenPr);
        assert!(plan.ticked(ShipStep::Push) && plan.ticked(ShipStep::OpenPr));
        plan.route = Route::Merge;
        assert_eq!(plan.steps(), [ShipStep::Merge]);
        // Its own push, not the pull request route's.
        plan.toggle(ShipStep::PushBranch);
        assert_eq!(plan.steps(), [ShipStep::PushBranch, ShipStep::Merge]);
        plan.toggle(ShipStep::PushBranch);
        assert!(plan.ticked(ShipStep::Push));
        plan.toggle(ShipStep::PushBase);
        assert_eq!(plan.steps(), [ShipStep::Merge, ShipStep::PushBase]);
        plan.toggle(ShipStep::Merge);
        assert!(plan.steps().is_empty());
        plan.toggle(ShipStep::PushBase);
        assert_eq!(plan.steps(), [ShipStep::Merge, ShipStep::PushBase]);
        // Another route's ticks wait.
        plan.route = Route::Pr;
        assert_eq!(plan.steps(), [ShipStep::Push, ShipStep::OpenPr]);
    }

    #[test]
    fn actions_read_as_words() {
        let api = || "api".to_owned();
        let labels: Vec<String> = [
            Action::Push { repo: api() },
            Action::OpenPr {
                repo: api(),
                base: "dev".into(),
            },
            Action::KeepPr {
                repo: api(),
                url: "https://x/1".into(),
            },
            Action::Squash {
                repo: api(),
                base: "main".into(),
            },
            Action::PushBase {
                repo: api(),
                base: "main".into(),
            },
        ]
        .iter()
        .map(|a| a.label("feature/x", false))
        .collect();
        assert_eq!(
            labels,
            [
                "Push feature/x of api to origin",
                "Open a pull request into dev for api",
                "Keep the pull request open for api: https://x/1",
                "Squash feature/x into main in api",
                "Push main of api to origin",
            ]
        );
        // One repository: as the Ship page's rows say it.
        let push = Action::Push { repo: api() };
        assert_eq!(push.label("feature/x", true), "Push feature/x to origin");
    }
}
