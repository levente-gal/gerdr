//! Features: a unit of work gerdr takes through nine stages, from a chat
//! about what to build to shipping it, each in its own git worktrees.
//!
//! A feature lives in two places:
//!
//! - its record, `feature.json`, and later its stage files (brief, plan,
//!   evidence), in gerdr's data folder:
//!   `~/.local/share/gerdr/features/<project>/<slug>/` (see `store`);
//! - its code, in a folder beside the project's repositories:
//!   `<root>/.features/<slug>/`, kept out of git (see `layout`). In a
//!   single repository that folder is the feature's worktree. Over several
//!   repositories it holds one worktree per repository, named like the
//!   repository: the ones the feature changes ("touched") on branch
//!   `feature/<slug>`, the others detached at their base, as context.
//!
//! Stages are manual ticks for now: the user marks one done in the feature
//! panel and gerdr records when. Later increments run the stages' agents
//! and checks, and fill the ticks themselves; the record keeps its shape.
//!
//! The ticks from Build on are bound to the code they were given on: the
//! feature's patch-id (see `patch`). When the code changes, those ticks no
//! longer hold, and the feature goes back to Build (`Feature::invalidate`).
//! Ship is never a tick: gerdr ships the feature (see `ship`) once every
//! other stage holds, by opening pull requests or by squashing it into the
//! base branch, and a `pre-push` hook (see `hook`) refuses to push a
//! feature's branch that gerdr didn't clear.

#![allow(dead_code)]

pub mod agents;
pub mod artifacts;
pub mod graph;
pub mod hook;
pub mod layout;
pub mod manual;
pub mod page;
pub mod patch;
pub mod plan_review;
pub mod review_import;
pub mod setup;
pub mod ship;
pub mod store;
pub mod verify;
pub mod viewed;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// The record's format; bumped when a field changes meaning.
pub const VERSION: u32 = 2;

/// The stages, in order. Every one has to be done; only Plan review can be
/// done by skipping it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Discover,
    Plan,
    PlanReview,
    Build,
    Verify,
    AiReview,
    CodeReview,
    ManualTest,
    Ship,
}

impl Stage {
    pub const ALL: [Self; 9] = [
        Self::Discover,
        Self::Plan,
        Self::PlanReview,
        Self::Build,
        Self::Verify,
        Self::AiReview,
        Self::CodeReview,
        Self::ManualTest,
        Self::Ship,
    ];

    /// The stage after it.
    pub fn next(self) -> Option<Self> {
        Self::ALL.into_iter().find(|s| *s > self)
    }

    /// Its name in the panel: "AI review".
    pub fn label(self) -> &'static str {
        match self {
            Self::Discover => "Discover",
            Self::Plan => "Plan",
            Self::PlanReview => "Plan review",
            Self::Build => "Build",
            Self::Verify => "Verify",
            Self::AiReview => "AI review",
            Self::CodeReview => "Code review",
            Self::ManualTest => "Manual test",
            Self::Ship => "Ship",
        }
    }

    /// One line on what is done in it, for the panel and its tooltip.
    pub fn about(self) -> &'static str {
        match self {
            Self::Discover => "Talk it through with an agent; agree on a brief",
            Self::Plan => "An agent plans the change; you approve the plan",
            Self::PlanReview => "Another agent checks the plan; you pick what goes in",
            Self::Build => "An agent builds it in the feature's worktrees",
            Self::Verify => "Tests and checks pass on the current code",
            Self::AiReview => "Another model reviews the diff",
            Self::CodeReview => "You review every changed file",
            Self::ManualTest => "You try it and check the brief's criteria",
            Self::Ship => "Merge into main, or open the pull requests",
        }
    }

    /// Whether its mark is bound to the code: holds only while the
    /// feature's patch-id is the one it was given on. Build and every
    /// stage after it.
    pub fn bound(self) -> bool {
        self >= Self::Build
    }
}

/// How a project's features ship, from its store file's
/// `"workflow": {"ship": ...}`; without it, per repository: a pull request
/// when its `origin` is on GitHub, else a squash into its base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShipMode {
    /// Push `feature/<slug>` and open a pull request into the base.
    Pr,
    /// Squash `feature/<slug>` into the base in the main worktree.
    Squash,
}

/// Where a feature stands as a whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Being worked on: shows in the sidebar.
    #[default]
    Open,
    /// Shipped: every stage ticked.
    Done,
    /// Given up; its worktrees are gone.
    Abandoned,
}

/// A stage marked done.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// Unix seconds.
    pub at: u64,
    /// Who marked it: "you" for a manual tick; later an agent or "gerdr".
    pub by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The feature's patch-id when it was marked, for a bound stage (see
    /// `Stage::bound`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

/// A pull request a feature opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    /// The repository's `Repo::key`.
    pub repo: String,
    pub url: String,
}

/// How far shipping a feature got.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shipping {
    /// The pull requests opened, in the order they were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prs: Vec<Pr>,
    /// When a squash into the base started in a tab (unix seconds): the
    /// feature is done once its branches read as merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squash_at: Option<u64>,
}

impl Shipping {
    pub fn is_empty(&self) -> bool {
        self.prs.is_empty() && self.squash_at.is_none()
    }
}

/// What names a repository among all a feature's: its `name` for a home
/// repository, `<project>:<name>` for a linked project's.
pub fn repo_key(project: Option<&str>, name: &str) -> String {
    match project {
        Some(project) => format!("{project}:{name}"),
        None => name.to_owned(),
    }
}

/// A repository the feature's agents may read but don't change: its main
/// checkout, with no worktree. Apart from `Feature::repos` so nothing that
/// walks those (setup, Verify, Ship, taking worktrees down) can ever act on
/// a main checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRepo {
    pub name: String,
    /// Its main checkout.
    pub main: PathBuf,
    /// The id of the project it belongs to when that isn't the home one.
    /// That project needs no link: nothing of the feature is in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl ContextRepo {
    /// See `repo_key`.
    pub fn key(&self) -> String {
        repo_key(self.project.as_deref(), &self.name)
    }
}

/// A repository taken out of the feature, kept so it can be added back.
/// Its worktree and branch are gone: adding it back makes new ones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedRepo {
    pub name: String,
    /// The id of the project it belongs to when that isn't the home one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// When it was taken out: Unix seconds.
    pub at: u64,
}

impl RemovedRepo {
    /// See `repo_key`.
    pub fn key(&self) -> String {
        repo_key(self.project.as_deref(), &self.name)
    }
}

/// One of the project's repositories, as the feature has it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    /// Its folder's name, which its worktree in the feature folder takes
    /// too: `services-api`. For a single repository, its folder's name.
    pub name: String,
    /// Its main worktree: where it is checked out outside the feature.
    pub main: PathBuf,
    /// The branch the feature starts from and merges into: `dev`, `main`.
    pub base: String,
    /// The commit `base` was at when the worktree was made.
    pub base_commit: String,
    /// Its worktree for the feature.
    pub worktree: PathBuf,
    /// Whether the feature changes it: then its worktree is on `branch`.
    /// Every repository linked to a feature is; a feature made before that
    /// can hold others, read-only and detached at `base_commit`.
    pub touched: bool,
    /// `feature/<slug>` once touched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The id of the project it belongs to when that is a linked one (see
    /// `Link`); `None` for the feature's home project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Repo {
    /// What names it among all the feature's repositories: its `name` for a
    /// home repository, `<project>:<name>` for a linked project's. Maps and
    /// lists keyed by repository use it.
    pub fn key(&self) -> String {
        repo_key(self.project.as_deref(), &self.name)
    }
}

/// Another project a feature spans: its own feature folder, made like the
/// home one, under its own root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// The project's id.
    pub project: String,
    /// Like `Feature::root`, for this project.
    pub root: PathBuf,
    /// `<root>/.features/<slug>`.
    pub dir: PathBuf,
    /// Whether `root` holds several repositories.
    pub multi: bool,
}

/// Something that happened to a feature, kept in its history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Unix seconds.
    pub at: u64,
    /// What happened, in words: "Plan ticked", "touched services-api".
    pub what: String,
}

/// A feature: its record, `feature.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    #[serde(default)]
    pub version: u32,
    /// Unique within the project; names its folders and branches.
    pub slug: String,
    pub title: String,
    /// The project's id.
    pub project: String,
    /// Unix seconds.
    pub created: u64,
    /// The folder holding the repositories: the repository itself for a
    /// single one, else their common parent.
    pub root: PathBuf,
    /// `<root>/.features/<slug>`.
    pub dir: PathBuf,
    /// Whether `root` holds several repositories.
    pub multi: bool,
    /// The repositories it holds: the home project's in name order, then
    /// each link's in link order, by name within it. Not every repository
    /// of a project: one comes in when it is linked to the feature.
    pub repos: Vec<Repo>,
    /// The other projects it spans, in the order they were linked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    /// The repositories its agents may read but don't change, in the order
    /// they were added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextRepo>,
    /// The repositories taken out of it and not added back, latest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<RemovedRepo>,
    #[serde(default)]
    pub status: Status,
    /// The stages marked done.
    #[serde(default)]
    pub marks: BTreeMap<Stage, Mark>,
    #[serde(default)]
    pub history: Vec<Event>,
    #[serde(default, skip_serializing_if = "Shipping::is_empty")]
    pub shipping: Shipping,
    /// The agent stages' latest runs (see `agents`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runs: BTreeMap<Stage, agents::StageRun>,
    /// The agent picked on a stage's page, by the name the picker gives
    /// it; over the project's `workflow.agents`, and only where it differs
    /// from what that gives.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stage_agents: BTreeMap<Stage, String>,
    /// Its worktrees' setup, once started (see `setup`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<setup::SetupRun>,
    /// Its latest Verify run (see `verify`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<verify::VerifyRun>,
    /// Its AI review's findings and what was decided of them (see
    /// `review_import`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<review_import::AiReview>,
    /// Plan review's suggestions and what was decided of them (see
    /// `plan_review`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_review: Option<plan_review::PlanReview>,
    /// Code review's viewed ticks (see `viewed`).
    #[serde(default, skip_serializing_if = "viewed::Viewed::is_empty")]
    pub viewed: viewed::Viewed,
    /// Manual test's results (see `manual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<manual::ManualTest>,
    /// The last run number of each stage whose run was forgotten: a later
    /// run goes on from it, so its prompt doesn't overwrite an old one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub run_counts: BTreeMap<Stage, u32>,
}

/// How a stage stands once it comes to changing it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Standing {
    /// Not done: being worked on, or not reached yet.
    #[default]
    Open,
    /// Done, and it can still be reopened.
    Approved,
    /// Done, and this later stage holds it: that one is done, or it has
    /// made something from this one's work.
    Locked(Stage),
}

impl Feature {
    /// Bring a record of an older format up to this one; whether it was
    /// older. A feature that went past Plan before Plan review existed has
    /// that stage marked done, at Plan's time, so it stays where it was.
    pub fn migrate(&mut self, now: u64) -> bool {
        if self.version >= VERSION {
            return false;
        }
        let built = self.marks.contains_key(&Stage::Build) || self.runs.contains_key(&Stage::Build);
        if self.version < 2
            && built
            && let Some(at) = self.marks.get(&Stage::Plan).map(|m| m.at)
        {
            self.marks.insert(
                Stage::PlanReview,
                Mark {
                    at,
                    by: "gerdr".to_owned(),
                    note: "before this stage existed".to_owned(),
                    patch: None,
                },
            );
            self.log(now, "Plan review marked done: the feature was past it");
        }
        self.version = VERSION;
        true
    }

    /// The first stage not done yet: the one being worked on. `None` once
    /// every stage is done.
    pub fn current(&self) -> Option<Stage> {
        Stage::ALL.into_iter().find(|s| !self.marks.contains_key(s))
    }

    /// Whether `stage` is done.
    pub fn is_done(&self, stage: Stage) -> bool {
        self.marks.contains_key(&stage)
    }

    /// Mark the current stage done at `now`, by `by`, on the code whose
    /// patch-id is `patch` (kept for a bound stage only); when it is Ship,
    /// the feature is done. Only the current stage can be ticked: stages
    /// go in order. Returns the stage ticked, or `None` when there is none.
    pub fn tick(&mut self, now: u64, by: &str, patch: Option<&str>) -> Option<Stage> {
        let stage = self.current()?;
        self.marks.insert(
            stage,
            Mark {
                at: now,
                by: by.to_owned(),
                note: String::new(),
                patch: patch.filter(|_| stage.bound()).map(str::to_owned),
            },
        );
        if stage == Stage::Ship {
            self.status = Status::Done;
        }
        self.log(now, format!("{} ticked", stage.label()));
        Some(stage)
    }

    /// Clear `stage`'s mark and every later one's, as when it has to be
    /// done again. A done feature opens again. Returns whether anything
    /// was cleared.
    pub fn untick(&mut self, stage: Stage, now: u64) -> bool {
        let before = self.marks.len();
        self.marks.retain(|s, _| *s < stage);
        if self.marks.len() == before {
            return false;
        }
        if self.status == Status::Done {
            self.status = Status::Open;
        }
        self.log(now, format!("{} to do again", stage.label()));
        true
    }

    /// The first bound stage marked on other code than `patch`'s: from it
    /// on, the marks no longer hold.
    pub fn stale(&self, patch: &str) -> Option<Stage> {
        Stage::ALL.into_iter().filter(|s| s.bound()).find(|s| {
            self.marks
                .get(s)
                .is_some_and(|m| m.patch.as_deref() != Some(patch))
        })
    }

    /// The code is now `patch`'s: clear the marks that no longer hold, from
    /// the first stale stage on, and log it ("Code changed: Build to do
    /// again"). A done feature is left as it is: it shipped. Returns the
    /// stage it goes back to.
    pub fn invalidate(&mut self, patch: &str, now: u64) -> Option<Stage> {
        if matches!(self.status, Status::Done | Status::Abandoned) {
            return None;
        }
        let stage = self.stale(patch)?;
        self.marks.retain(|s, _| *s < stage);
        self.log(now, format!("Code changed: {} to do again", stage.label()));
        Some(stage)
    }

    /// Whether `stage` has made something that its starting over would
    /// lose. `wrote` says whether its file is there (see
    /// `artifacts::outputs`): a stage's own file, or the code review's
    /// comments.
    pub fn has_output(&self, stage: Stage, wrote: impl Fn(Stage) -> bool) -> bool {
        wrote(stage)
            || match stage {
                Stage::Discover | Stage::Plan | Stage::Build => false,
                Stage::PlanReview => self.plan_review.is_some(),
                Stage::Verify => self.verify.is_some(),
                Stage::AiReview => self.review.is_some(),
                Stage::CodeReview => !self.viewed.is_empty(),
                Stage::ManualTest => self.manual.is_some(),
                Stage::Ship => !self.shipping.is_empty(),
            }
    }

    /// The later stage that keeps done `stage` from being reopened: the
    /// next one once it is done too, else the current one once it has made
    /// something. Only those count: what a stage left behind before the
    /// code changed and sent the feature back doesn't hold an earlier one.
    pub fn locked_by(&self, stage: Stage, wrote: impl Fn(Stage) -> bool) -> Option<Stage> {
        if !self.is_done(stage) {
            return None;
        }
        let next = stage.next()?;
        if self.is_done(next) {
            return Some(next);
        }
        self.has_output(next, wrote).then_some(next)
    }

    /// How `stage` stands (see `Standing`).
    pub fn standing(&self, stage: Stage, wrote: impl Fn(Stage) -> bool) -> Standing {
        if !self.is_done(stage) {
            return Standing::Open;
        }
        match self.locked_by(stage, wrote) {
            Some(by) => Standing::Locked(by),
            None => Standing::Approved,
        }
    }

    /// The number the next run of `stage`'s agent takes.
    pub fn next_run(&self, stage: Stage) -> u32 {
        let last = self.runs.get(&stage).map(|r| r.n);
        let counted = self.run_counts.get(&stage).copied();
        last.max(counted).unwrap_or(0) + 1
    }

    /// Forget `stage`'s run, its number kept for the next one.
    pub fn forget_run(&mut self, stage: Stage) {
        if let Some(run) = self.runs.remove(&stage) {
            self.run_counts.insert(stage, run.n);
        }
    }

    /// Forget what current `stage` has made, as if it had just been
    /// reached: its run, and what it recorded. The files are its host's.
    /// Returns false for any other stage.
    pub fn start_over(&mut self, stage: Stage, now: u64) -> bool {
        if self.current() != Some(stage) {
            return false;
        }
        self.forget_run(stage);
        match stage {
            Stage::Discover | Stage::Plan | Stage::Build => {}
            Stage::PlanReview => self.plan_review = None,
            Stage::Verify => self.verify = None,
            Stage::AiReview => self.review = None,
            Stage::CodeReview => self.viewed = viewed::Viewed::default(),
            Stage::ManualTest => self.manual = None,
            Stage::Ship => self.shipping = Shipping::default(),
        }
        self.log(now, format!("{} started over", stage.label()));
        true
    }

    /// Add `what` to its history at `now`.
    pub fn log(&mut self, now: u64, what: impl Into<String>) {
        self.history.push(Event {
            at: now,
            what: what.into(),
        });
    }

    /// The repositories it changes.
    pub fn touched(&self) -> impl Iterator<Item = &Repo> {
        self.repos.iter().filter(|r| r.touched)
    }

    /// Its branch name: `feature/<slug>`.
    pub fn branch(&self) -> String {
        format!("feature/{}", self.slug)
    }

    /// The folders its agents are let into besides their working folder
    /// (`dir`): the feature data dir `data`, each linked project's feature
    /// folder, then each context repository's main checkout.
    pub fn agent_dirs(&self, data: &Path) -> Vec<PathBuf> {
        std::iter::once(data.to_path_buf())
            .chain(self.links.iter().map(|l| l.dir.clone()))
            .chain(self.context.iter().map(|c| c.main.clone()))
            .collect()
    }

    /// Keep `repo` as context: it is no longer one taken out.
    pub fn add_context(&mut self, repo: ContextRepo) {
        let key = repo.key();
        self.removed.retain(|r| r.key() != key);
        self.context.retain(|c| c.key() != key);
        self.context.push(repo);
    }

    /// Note that the repository `name` of `project` was taken out at `at`.
    pub fn note_removed(&mut self, project: Option<String>, name: String, at: u64) {
        let gone = RemovedRepo { name, project, at };
        let key = gone.key();
        self.removed.retain(|r| r.key() != key);
        self.removed.insert(0, gone);
    }

    /// Whether it has more than one repository, over all its projects.
    pub fn many_repos(&self) -> bool {
        self.multi || !self.links.is_empty()
    }

    /// The repository whose `Repo::key` is `key`.
    pub fn repo(&self, key: &str) -> Option<&Repo> {
        self.repos.iter().find(|r| r.key() == key)
    }

    /// Its link to project `project`, if it spans it.
    pub fn link(&self, project: &str) -> Option<&Link> {
        self.links.iter().find(|l| l.project == project)
    }

    /// The feature folder `repo` is in: the home `dir`, or its link's.
    pub fn dir_of(&self, repo: &Repo) -> &Path {
        repo.project
            .as_deref()
            .and_then(|p| self.link(p))
            .map_or(self.dir.as_path(), |l| l.dir.as_path())
    }

    /// Every project it spans with its feature folder: the home one
    /// (`None`) first, then each link in link order.
    pub fn folders(&self) -> Vec<(Option<&str>, &Path)> {
        std::iter::once((None, self.dir.as_path()))
            .chain(
                self.links
                    .iter()
                    .map(|l| (Some(l.project.as_str()), l.dir.as_path())),
            )
            .collect()
    }

    /// Its repositories of project `project` (`None`: the home one).
    pub fn repos_of<'a>(&'a self, project: Option<&'a str>) -> impl Iterator<Item = &'a Repo> {
        self.repos
            .iter()
            .filter(move |r| r.project.as_deref() == project)
    }

    /// Keep `new` among its repositories, in the order `repos` has them.
    /// They are no longer context or taken out.
    pub fn insert_repos(&mut self, new: Vec<Repo>) {
        let keys: Vec<String> = new.iter().map(Repo::key).collect();
        self.context.retain(|c| !keys.contains(&c.key()));
        self.removed.retain(|r| !keys.contains(&r.key()));
        let mut repos = std::mem::take(&mut self.repos);
        repos.extend(new);
        let rank = |r: &Repo| {
            r.project.as_deref().map_or(0, |id| {
                let at = self.links.iter().position(|l| l.project == id);
                at.map_or(self.links.len() + 1, |i| i + 1)
            })
        };
        repos.sort_by_cached_key(|r| (rank(r), r.name.clone()));
        self.repos = repos;
    }

    /// Its repositories in key order: the order patch-ids, setup and
    /// checks go in.
    pub fn repos_by_key(&self) -> Vec<&Repo> {
        let mut repos: Vec<&Repo> = self.repos.iter().collect();
        repos.sort_by_key(|r| r.key());
        repos
    }
}

/// A setting of a feature's projects' workflows, per project: the home
/// one's and each linked one's by id. `of` gives the one a repository's
/// own project sets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PerProject<T> {
    pub home: T,
    pub links: BTreeMap<String, T>,
}

impl<T> PerProject<T> {
    /// The same `home` for every project, as for a feature of one.
    pub fn all(home: T) -> Self {
        Self {
            home,
            links: BTreeMap::new(),
        }
    }

    /// `repo`'s own project's setting; a project without one (gone from
    /// the store) gives nothing.
    pub fn of(&self, repo: &Repo) -> Option<&T> {
        match &repo.project {
            None => Some(&self.home),
            Some(p) => self.links.get(p),
        }
    }
}

impl PerProject<String> {
    /// What `repo` is called on screen, with these projects' names: its
    /// name, a linked project's as "<project>: <repo>".
    pub fn label(&self, repo: &Repo) -> String {
        self.label_of(repo.project.as_deref(), &repo.name)
    }

    /// The same of repository `name` of `project`, held by the feature or
    /// not.
    pub fn label_of(&self, project: Option<&str>, name: &str) -> String {
        match project {
            None => name.to_owned(),
            Some(id) => {
                let of = self.links.get(id).map_or(id, |p| p.as_str());
                format!("{of}: {name}")
            }
        }
    }
}

/// Unix seconds now.
pub fn now() -> u64 {
    crate::review::now()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    /// `feature`, its stages up to `done` marked.
    fn done_up_to(done: Stage) -> Feature {
        let mut f = feature("x", 0);
        for stage in Stage::ALL.into_iter().filter(|s| *s <= done) {
            f.marks.insert(
                stage,
                Mark {
                    at: 1,
                    by: "you".into(),
                    note: String::new(),
                    patch: None,
                },
            );
        }
        f
    }

    #[test]
    fn a_done_stage_locks_once_the_next_has_made_something() {
        let f = done_up_to(Stage::Discover);
        let nothing = |_: Stage| false;
        let plan = |s: Stage| s == Stage::Plan;
        assert_eq!(f.standing(Stage::Plan, nothing), Standing::Open);
        assert_eq!(f.standing(Stage::Discover, nothing), Standing::Approved);
        assert_eq!(
            f.standing(Stage::Discover, plan),
            Standing::Locked(Stage::Plan)
        );
        // The next one done locks it, file or not.
        let f = done_up_to(Stage::Plan);
        assert_eq!(f.locked_by(Stage::Discover, nothing), Some(Stage::Plan));
        // Plan itself is still free while Build has made nothing: Build
        // writes no file, its run isn't output.
        assert_eq!(f.locked_by(Stage::Plan, nothing), None);
        // A record counts as much as a file.
        let mut f = done_up_to(Stage::ManualTest);
        assert_eq!(f.locked_by(Stage::ManualTest, nothing), None);
        f.shipping.prs.push(Pr {
            repo: "api".into(),
            url: "https://example.test/1".into(),
        });
        assert_eq!(f.locked_by(Stage::ManualTest, nothing), Some(Stage::Ship));
        // The last stage has none after it.
        let f = done_up_to(Stage::Ship);
        assert_eq!(f.standing(Stage::Ship, nothing), Standing::Approved);
    }

    #[test]
    fn what_a_stage_left_before_going_back_holds_nothing() {
        // Stale marks removed: Plan's file is there, Discover the current.
        let f = feature("x", 0);
        let plan = |s: Stage| s == Stage::Plan;
        assert_eq!(f.current(), Some(Stage::Discover));
        assert_eq!(f.standing(Stage::Discover, plan), Standing::Open);
    }

    #[test]
    fn starting_over_forgets_only_the_current_stage_and_keeps_counting() {
        let mut f = done_up_to(Stage::PlanReview);
        f.runs.insert(
            Stage::Build,
            agents::StageRun {
                n: 2,
                ..agents::StageRun::default()
            },
        );
        assert_eq!(f.next_run(Stage::Build), 3);
        // Only the current stage starts over.
        assert!(!f.start_over(Stage::Plan, 5));
        assert!(f.is_done(Stage::Plan));
        assert!(f.start_over(Stage::Build, 5));
        assert!(!f.runs.contains_key(&Stage::Build));
        // Its next run takes the next number, for chats kept apart.
        assert_eq!(f.next_run(Stage::Build), 3);
        assert_eq!(f.history.last().unwrap().what, "Build started over");
        // What it recorded goes.
        let mut f = done_up_to(Stage::ManualTest);
        f.shipping.prs.push(Pr {
            repo: "api".into(),
            url: "https://example.test/1".into(),
        });
        assert!(f.start_over(Stage::Ship, 5));
        assert!(f.shipping.is_empty());
        assert_eq!(f.standing(Stage::ManualTest, |_| false), Standing::Approved);
    }

    /// A folder of its own under the system's temporary folder, removed
    /// when dropped.
    pub(crate) struct Temp(pub(crate) PathBuf);

    impl Temp {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-feature-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }

        /// A repository `name` in it on branch `branch`, with a commit.
        pub(crate) fn repo(&self, name: &str, branch: &str) -> PathBuf {
            let dir = self.0.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            run(&dir, &["init", "-q", "-b", branch]);
            commit(&dir, "a", "first");
            dir
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `git -C dir args`, as features run it.
    pub(crate) fn try_run(dir: &Path, args: &[&str]) -> Result<String, String> {
        layout::git(dir, args)
    }

    pub(crate) fn run(dir: &Path, args: &[&str]) -> String {
        try_run(dir, args).unwrap_or_else(|err| panic!("git {args:?}: {err}"))
    }

    /// Write `text` to `file` in `dir` and commit it.
    pub(crate) fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-q", "-m", text]);
    }

    /// A feature over repositories `names` under `t`, all of them touched
    /// but those in `untouched`, each worktree on `feature/f` from main.
    pub(crate) fn feature_over(t: &Temp, names: &[&str], untouched: &[&str]) -> Feature {
        let dir = t.0.join(".features/f");
        std::fs::create_dir_all(&dir).unwrap();
        let repos = names
            .iter()
            .map(|name| {
                let main = t.repo(name, "main");
                let worktree = dir.join(name);
                let touched = !untouched.contains(name);
                let w = worktree.to_str().unwrap();
                if touched {
                    run(&main, &["worktree", "add", "-q", "-b", "feature/f", w]);
                } else {
                    run(&main, &["worktree", "add", "-q", "--detach", w]);
                }
                Repo {
                    name: (*name).into(),
                    base_commit: run(&main, &["rev-parse", "main"]).trim().into(),
                    main,
                    base: "main".into(),
                    worktree,
                    touched,
                    branch: touched.then(|| "feature/f".into()),
                    project: None,
                }
            })
            .collect();
        Feature {
            version: VERSION,
            slug: "f".into(),
            title: "Log in".into(),
            project: "p".into(),
            created: 1,
            root: t.0.clone(),
            dir,
            multi: true,
            repos,
            links: Vec::new(),
            status: Status::Open,
            marks: Default::default(),
            run_counts: Default::default(),
            context: Vec::new(),
            removed: Vec::new(),
            history: Vec::new(),
            shipping: Default::default(),
            runs: Default::default(),
            stage_agents: Default::default(),
            setup: None,
            verify: None,
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
        }
    }

    /// Link project `project` to `f`: repositories `names` in folder
    /// `<t>/<project>` (its root), touched but those in `untouched`, each
    /// worktree in `<root>/.features/f`, as `feature_over` makes them.
    pub(crate) fn link_over(
        t: &Temp,
        f: &mut Feature,
        project: &str,
        names: &[&str],
        untouched: &[&str],
    ) {
        let root = t.0.join(project);
        let dir = root.join(".features/f");
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            let main = root.join(name);
            std::fs::create_dir_all(&main).unwrap();
            run(&main, &["init", "-q", "-b", "main"]);
            commit(&main, "a", "first");
            let worktree = dir.join(name);
            let touched = !untouched.contains(name);
            let w = worktree.to_str().unwrap();
            if touched {
                run(&main, &["worktree", "add", "-q", "-b", "feature/f", w]);
            } else {
                run(&main, &["worktree", "add", "-q", "--detach", w]);
            }
            f.repos.push(Repo {
                name: (*name).into(),
                base_commit: run(&main, &["rev-parse", "main"]).trim().into(),
                main,
                base: "main".into(),
                worktree,
                touched,
                branch: touched.then(|| "feature/f".into()),
                project: Some(project.into()),
            });
        }
        f.links.push(Link {
            project: project.into(),
            root,
            dir,
            multi: true,
        });
    }

    pub(crate) fn feature(slug: &str, created: u64) -> Feature {
        Feature {
            version: VERSION,
            slug: slug.into(),
            title: format!("Feature {slug}"),
            project: "p".into(),
            created,
            root: PathBuf::from("/r"),
            dir: PathBuf::from("/r/.features").join(slug),
            multi: false,
            repos: Vec::new(),
            links: Vec::new(),
            status: Status::Open,
            marks: BTreeMap::new(),
            history: Vec::new(),
            shipping: Shipping::default(),
            run_counts: BTreeMap::new(),
            context: Vec::new(),
            removed: Vec::new(),
            runs: BTreeMap::new(),
            stage_agents: BTreeMap::new(),
            setup: None,
            verify: None,
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
        }
    }

    #[test]
    fn ticks_go_in_order_and_ship_finishes() {
        let mut f = feature("a", 1);
        assert_eq!(f.current(), Some(Stage::Discover));
        assert_eq!(f.tick(10, "you", None), Some(Stage::Discover));
        assert_eq!(f.tick(11, "you", None), Some(Stage::Plan));
        assert!(f.is_done(Stage::Plan));
        assert_eq!(f.current(), Some(Stage::PlanReview));
        assert_eq!(f.marks[&Stage::Plan].at, 11);
        assert_eq!(f.marks[&Stage::Plan].by, "you");
        assert_eq!(f.history.last().unwrap().what, "Plan ticked");
        for _ in 2..8 {
            f.tick(12, "you", None).unwrap();
            assert_eq!(f.status, Status::Open);
        }
        assert_eq!(f.tick(13, "you", None), Some(Stage::Ship));
        assert_eq!(f.status, Status::Done);
        assert_eq!(f.tick(14, "you", None), None);
        assert_eq!(f.history.len(), 9);
    }

    #[test]
    fn untick_clears_later_stages_and_reopens() {
        let mut f = feature("a", 1);
        for _ in 0..9 {
            f.tick(1, "you", None);
        }
        assert_eq!(f.status, Status::Done);
        assert!(f.untick(Stage::AiReview, 5));
        assert_eq!(f.status, Status::Open);
        assert_eq!(f.current(), Some(Stage::AiReview));
        assert!(f.is_done(Stage::Verify));
        assert!(!f.is_done(Stage::Ship));
        assert_eq!(f.history.last().unwrap().what, "AI review to do again");
        let len = f.history.len();
        assert!(!f.untick(Stage::Ship, 6));
        assert_eq!(f.history.len(), len);
    }

    #[test]
    fn a_code_change_sends_bound_marks_back() {
        let mut f = feature("a", 1);
        f.tick(1, "you", Some("p1"));
        f.tick(1, "you", Some("p1"));
        f.tick(1, "you", Some("p1"));
        // The stages before Build keep no patch: they hold whatever the code.
        assert!(f.marks[&Stage::Plan].patch.is_none());
        assert_eq!(f.stale("p2"), None);
        f.tick(2, "you", Some("p1"));
        f.tick(3, "you", Some("p2"));
        assert_eq!(f.stale("p1"), Some(Stage::Verify));
        assert_eq!(f.stale("p2"), Some(Stage::Build));
        assert_eq!(f.invalidate("p1", 5), Some(Stage::Verify));
        assert_eq!(f.current(), Some(Stage::Verify));
        assert!(f.is_done(Stage::Build));
        assert_eq!(
            f.history.last().unwrap().what,
            "Code changed: Verify to do again"
        );
        assert_eq!(f.invalidate("p1", 6), None);
        // A mark without a patch never held for any code.
        f.marks.get_mut(&Stage::Build).unwrap().patch = None;
        assert_eq!(f.stale("p1"), Some(Stage::Build));
    }

    #[test]
    fn a_done_feature_is_not_invalidated() {
        let mut f = feature("a", 1);
        for _ in 0..9 {
            f.tick(1, "you", Some("p1"));
        }
        assert_eq!(f.status, Status::Done);
        assert_eq!(f.invalidate("p2", 2), None);
        assert_eq!(f.current(), None);
        f.status = Status::Abandoned;
        assert_eq!(f.invalidate("p2", 2), None);
    }

    #[test]
    fn linked_repos_have_their_own_keys_and_folders() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        link_over(&t, &mut f, "other", &["api", "web"], &["web"]);
        let keys: Vec<String> = f.repos.iter().map(Repo::key).collect();
        assert_eq!(keys, ["api", "other:api", "other:web"]);
        assert!(f.many_repos());
        assert_eq!(f.repo("other:web").unwrap().name, "web");
        assert!(f.repo("web").is_none());
        let linked = f.repo("other:api").unwrap();
        assert_eq!(f.dir_of(linked), t.0.join("other/.features/f"));
        assert_eq!(f.dir_of(&f.repos[0]), f.dir);
        let folders: Vec<_> = f.folders().into_iter().map(|(p, _)| p).collect();
        assert_eq!(folders, [None, Some("other")]);
        assert_eq!(f.repos_of(Some("other")).count(), 2);
        assert_eq!(
            f.agent_dirs(Path::new("/data")),
            [PathBuf::from("/data"), t.0.join("other/.features/f")]
        );
        let modes = PerProject {
            home: 1,
            links: [("other".to_owned(), 2)].into(),
        };
        assert_eq!(modes.of(&f.repos[0]), Some(&1));
        assert_eq!(modes.of(linked), Some(&2));
        // An old record, without links or projects, loads as it was.
        let old = r#"{"slug":"a","title":"A","project":"p","created":1,"root":"/r",
            "dir":"/r/.features/a","multi":false,"repos":[{"name":"r","main":"/r",
            "base":"main","base_commit":"c","worktree":"/r/.features/a","touched":true}]}"#;
        let old: Feature = serde_json::from_str(old).unwrap();
        assert!(old.links.is_empty() && old.repos[0].project.is_none());
        assert!(old.context.is_empty() && old.removed.is_empty());
        assert_eq!(old.repos[0].key(), "r");
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<Feature>(&json).unwrap(), f);
    }

    #[test]
    fn log_adds_an_event() {
        let mut f = feature("a", 1);
        f.log(3, "touched x");
        assert_eq!(
            f.history,
            vec![Event {
                at: 3,
                what: "touched x".into()
            }]
        );
    }

    #[test]
    fn context_and_removed_stay_apart_and_an_added_repository_leaves_both() {
        let mut f = feature("a", 1);
        let docs = ContextRepo {
            name: "docs".into(),
            main: "/r/docs".into(),
            project: None,
        };
        f.note_removed(None, "auth".into(), 5);
        f.note_removed(Some("ops".into()), "infra".into(), 6);
        assert_eq!(f.removed[0].key(), "ops:infra", "latest first");
        f.add_context(docs.clone());
        f.add_context(docs.clone());
        assert_eq!(f.context.len(), 1);
        let data = Path::new("/data/a");
        assert_eq!(
            f.agent_dirs(data),
            [PathBuf::from("/data/a"), PathBuf::from("/r/docs")]
        );
        // A removed one added as context is no longer removed.
        f.add_context(ContextRepo {
            name: "auth".into(),
            main: "/r/auth".into(),
            project: None,
        });
        assert_eq!(f.removed.len(), 1);
        // One added to the feature is neither.
        let repo = |name: &str| Repo {
            name: name.into(),
            main: PathBuf::new(),
            base: "main".into(),
            base_commit: String::new(),
            worktree: PathBuf::new(),
            touched: true,
            branch: None,
            project: None,
        };
        f.note_removed(None, "web".into(), 7);
        f.insert_repos(vec![repo("web"), repo("docs")]);
        assert!(!f.removed.iter().any(|r| r.key() == "web"));
        assert!(!f.context.iter().any(|c| c.key() == "docs"));
        // It survives the record, and a record without them reads as empty.
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<Feature>(&json).unwrap(), f);
        let mut bare = feature("a", 1);
        bare.context.clear();
        let json = serde_json::to_string(&bare).unwrap();
        assert!(!json.contains("context") && !json.contains("removed"));
    }
}
