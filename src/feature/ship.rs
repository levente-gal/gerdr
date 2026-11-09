//! Shipping a feature, and what stops it.
//!
//! `probe` reads a feature for the panel, off the UI thread: its patch-id,
//! its guards, and when Ship is its stage, what can stop each step of it
//! (`ShipCheck`). Which steps run is the page's ticks (`ShipPlan`), so
//! `plan_actions` says what shipping does from the two, with no new probe.
//! Each repository with commits goes its own way (`Route`):
//!
//! - through origin (`open_prs`), in the feature's order (home project
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
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
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
    /// In words, for the Ship sheet: "Push services-api", "Open a pull
    /// request into dev for services-api". `Action` doesn't carry the
    /// branch: the sheet says it once, above the list.
    pub fn label(&self) -> String {
        match self {
            Self::Push { repo } => format!("Push {repo}"),
            Self::OpenPr { repo, base } => {
                format!("Open a pull request into {base} for {repo}")
            }
            Self::KeepPr { repo, url } => {
                format!("Keep the pull request open for {repo}: {url}")
            }
            Self::LinkPrs => "Link the pull requests to each other".into(),
            Self::Squash { repo, base } => format!("Squash the feature into {base} in {repo}"),
            Self::PushBase { repo, base } => format!("Push {base} of {repo} to origin"),
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

/// Whether `step` of `repo` ran already and needn't again: a push only while
/// the branch is where it was pushed from, a squash only while it reads as
/// merged.
pub fn step_done(feature: &Feature, repo: &RepoCheck, step: ShipStep) -> Option<super::ShipDone> {
    let done = feature.ship_done(&repo.key, step)?;
    let holds = match step {
        ShipStep::Push | ShipStep::PushBranch => {
            done.sha.is_none() || done.sha.as_deref() == repo.head.as_deref()
        }
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
    let guards = feature
        .touched()
        .map(|repo| (repo.key(), guard_of(repo, &branch)))
        .collect();
    let ship = feature.ships().then(|| check(feature, &reading, modes));
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
        });
    }
    out
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
                match push(r, &branch) {
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

/// The repository `action` acts on, or "" for one on all.
fn action_repo(action: &Action) -> &str {
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
    let text = std::fs::read_to_string(data.join(super::SUMMARY_FILE)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Push `branch` of `repo` to origin with its upstream, allowed for its
/// HEAD alone; guarded again after. The commit pushed.
fn push(repo: &Repo, branch: &str) -> Result<String, String> {
    let head = git(&repo.worktree, &["rev-parse", "HEAD"])?;
    let head = head.trim().to_owned();
    hook::allow(&repo.main, branch, &head)?;
    let pushed = git_ops::push(&repo.worktree, Some("origin"), false);
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
fn run_gh(gh: &str, dir: &Path, args: &[&str]) -> Result<String, String> {
    let what = args.get(..2).unwrap_or(args).join(" ");
    let mut command = Command::new(gh);
    // The real gh, not a test's.
    if gh == "gh" {
        crate::ci::signed_in(&mut command)?;
    }
    let mut child = command
        .args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => "gh is not installed".to_owned(),
            _ => format!("couldn't run gh: {err}"),
        })?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                // Cut short when gh dies: what came is what there is.
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as _));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < GH_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => {
                // Killed: a gh gone already is fine.
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    match status {
        None => Err(format!(
            "gh {what} took longer than {} s",
            GH_TIMEOUT.as_secs()
        )),
        Some(status) if status.success() => Ok(stdout),
        Some(_) => {
            let why = stderr.lines().map(str::trim).find(|l| !l.is_empty());
            Err(why.map_or_else(|| format!("gh {what} failed"), str::to_owned))
        }
    }
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

/// `text` quoted for a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
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
            check.actions[4].label(),
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
                "cd '{}' && git merge --squash 'feature/f' && git commit -e -m 'Don'\\''t break'",
                main.display()
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
        assert!(command.starts_with(&format!("cd '{}' &&", linked.display())));

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
        .map(Action::label)
        .collect();
        assert_eq!(
            labels,
            [
                "Push api",
                "Open a pull request into dev for api",
                "Keep the pull request open for api: https://x/1",
                "Squash the feature into main in api",
                "Push main of api to origin",
            ]
        );
    }
}
