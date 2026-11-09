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
//! base branch, as its page ticks them (`ShipPlan`), and a `pre-push` hook
//! (see `hook`) refuses to push a feature's branch that gerdr didn't clear.
//! With no step ticked, it finishes unshipped once the user says so.

#![allow(dead_code)]

pub mod agents;
pub mod artifacts;
pub mod drafts;
pub mod fix_round;
pub mod graph;
pub mod hook;
pub mod layout;
pub mod lease;
pub mod manual;
pub mod page;
pub mod patch;
pub mod plan_review;
pub mod remote;
pub mod review_import;
pub mod review_items;
pub mod sandbox;
pub mod sandbox_pages;
pub mod sandbox_run;
pub mod setup;
pub mod ship;
pub mod store;
pub mod suggest;
pub mod verify;
pub mod view;
pub mod viewed;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// The record's format; bumped when a field changes meaning. 3: its history
/// and its AI review's earlier rounds are kept in files of their own beside
/// it (see `store`).
pub const VERSION: u32 = 3;

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
            Self::Ship => "Open the pull requests, or merge into the base branch",
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
    /// The repositories whose branch was pushed, by `Repo::key`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pushed: Vec<String>,
    /// The repositories that squash, by `Repo::key`: those the merge waits
    /// for. A record from before these were kept has none: then every
    /// touched repository with commits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub squash_repos: Vec<String>,
    /// Those of them whose base goes to origin once merged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push_base: Vec<String>,
}

impl Shipping {
    pub fn is_empty(&self) -> bool {
        self.prs.is_empty()
            && self.squash_at.is_none()
            && self.pushed.is_empty()
            && self.squash_repos.is_empty()
            && self.push_base.is_empty()
    }
}

/// The way a repository goes when its feature ships, as Ship's page picks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    /// Through origin: `RepoPlan::push`, `RepoPlan::open_pr`.
    Pr,
    /// Into the base in its main worktree: `RepoPlan::push_branch`,
    /// `RepoPlan::merge`, `RepoPlan::push_base`.
    Merge,
    /// It stays as it is.
    Nothing,
}

impl Route {
    /// In the order the page's choice has them.
    pub const ALL: [Self; 3] = [Self::Pr, Self::Merge, Self::Nothing];

    /// The route a repository takes without a pick: how its project ships.
    pub fn of_mode(mode: ShipMode) -> Self {
        match mode {
            ShipMode::Pr => Self::Pr,
            ShipMode::Squash => Self::Merge,
        }
    }

    /// Its tick boxes, in the order they run.
    pub fn steps(self) -> &'static [ShipStep] {
        match self {
            Self::Pr => &[ShipStep::Push, ShipStep::OpenPr],
            Self::Merge => &[ShipStep::PushBranch, ShipStep::Merge, ShipStep::PushBase],
            Self::Nothing => &[],
        }
    }

    /// The one after it in the page's choice, round to the first.
    pub fn next(self) -> Self {
        match self {
            Self::Pr => Self::Merge,
            Self::Merge => Self::Nothing,
            Self::Nothing => Self::Pr,
        }
    }

    /// Its place in `ALL`.
    pub fn index(self) -> usize {
        match self {
            Self::Pr => 0,
            Self::Merge => 1,
            Self::Nothing => 2,
        }
    }
}

/// A tick box of a repository's steps on Ship's page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShipStep {
    Push,
    OpenPr,
    /// Push `feature/<slug>` to origin on the merge route: the same push as
    /// `Push`, ticked apart from it.
    PushBranch,
    Merge,
    PushBase,
}

/// What a repository does when its feature ships. Every route's ticks are
/// kept, so going back to a route finds them as they were.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoPlan {
    pub route: Route,
    /// Push `feature/<slug>` to origin.
    pub push: bool,
    /// Open a pull request into the base: needs `push`.
    pub open_pr: bool,
    /// Push `feature/<slug>` to origin too when it merges.
    #[serde(default)]
    pub push_branch: bool,
    /// Squash `feature/<slug>` into the base in its main worktree.
    pub merge: bool,
    /// Push the base to origin after: needs `merge`.
    pub push_base: bool,
}

impl RepoPlan {
    /// What a repository taking `route` does until a tick says otherwise.
    pub fn new(route: Route) -> Self {
        Self {
            route,
            push: true,
            open_pr: true,
            push_branch: false,
            merge: true,
            push_base: false,
        }
    }

    /// Whether `step` is ticked.
    pub fn ticked(&self, step: ShipStep) -> bool {
        match step {
            ShipStep::Push => self.push,
            ShipStep::OpenPr => self.push && self.open_pr,
            ShipStep::PushBranch => self.push_branch,
            ShipStep::Merge => self.merge,
            ShipStep::PushBase => self.merge && self.push_base,
        }
    }

    /// Flip `step`, and what depends on it or it depends on.
    pub fn toggle(&mut self, step: ShipStep) {
        let on = !self.ticked(step);
        match step {
            ShipStep::Push => {
                self.push = on;
                if !on {
                    self.open_pr = false;
                }
            }
            ShipStep::OpenPr => {
                self.open_pr = on;
                if on {
                    self.push = true;
                }
            }
            ShipStep::PushBranch => self.push_branch = on,
            ShipStep::Merge => {
                self.merge = on;
                if !on {
                    self.push_base = false;
                }
            }
            ShipStep::PushBase => {
                self.push_base = on;
                if on {
                    self.merge = true;
                }
            }
        }
    }

    /// The steps of its route that are ticked, in the order they run.
    pub fn steps(&self) -> Vec<ShipStep> {
        (self.route.steps().iter().copied())
            .filter(|s| self.ticked(*s))
            .collect()
    }
}

/// What the ticks on Ship's page say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipPlan {
    /// By `Repo::key`: a repository not here takes its default route, all
    /// its ticks as `RepoPlan::new` has them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repos: BTreeMap<String, RepoPlan>,
    /// The workspace stays open once shipped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_workspace: bool,
    /// Its worktrees and branches go once shipped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_worktrees: bool,
}

impl ShipPlan {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// What repository `key` does, `default` being its route without a pick.
    pub fn repo(&self, key: &str, default: Route) -> RepoPlan {
        (self.repos.get(key).cloned()).unwrap_or_else(|| RepoPlan::new(default))
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
    /// When the stages whose marks went were last marked done, until they
    /// are again: they ran once, and the rail says so.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ran_once: BTreeMap<Stage, u64>,
    /// What happened to it, oldest first. Kept in `store::HISTORY`, not in
    /// `feature.json`: `store` takes it out when it saves and puts it back
    /// when it reads. An older gerdr's record still has it here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Event>,
    #[serde(default, skip_serializing_if = "Shipping::is_empty")]
    pub shipping: Shipping,
    /// What its Ship page's ticks say. Not `shipping`: ticks alone are no
    /// work of the stage.
    #[serde(default, skip_serializing_if = "ShipPlan::is_empty")]
    pub ship_plan: ShipPlan,
    /// The agent stages' latest runs (see `agents`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runs: BTreeMap<Stage, agents::StageRun>,
    /// The agent picked on a stage's page, by the name the picker gives
    /// it; over the project's `workflow.agents`, and only where it differs
    /// from what that gives.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stage_agents: BTreeMap<Stage, String>,
    /// The model picked on a stage's page, by the agent it is for (its
    /// picker name): none starts the agent on its own default. Each agent
    /// keeps its own, as a model of one means nothing to another.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stage_models: BTreeMap<Stage, BTreeMap<String, String>>,
    /// The machine picked on a stage's page, by its id (`machines::Site::id`)
    /// or `machine_work::HERE`; over the project's `workflow.machines`, and
    /// only where it differs from what that gives.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stage_machines: BTreeMap<Stage, String>,
    /// The last setup on each other place than where `setup` ran: each has
    /// worktrees of its own to set up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setups_elsewhere: Vec<setup::SetupRun>,
    /// Build starts a fresh agent for each milestone, instead of one agent
    /// going on through them all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fresh_milestones: bool,
    /// Its worktrees' setup, once started (see `setup`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<setup::SetupRun>,
    /// Checks only this feature runs, after the ones it borrows from its
    /// projects, by `Repo::key` or `verify::FEATURE_KEY` (see `verify`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<String, Vec<String>>,
    /// The borrowed checks this feature doesn't run, by the same keys, as
    /// the project words them (see `verify`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skip_checks: BTreeMap<String, Vec<String>>,
    /// Its latest Verify run (see `verify`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<verify::VerifyRun>,
    /// Its sandbox, once started (see `sandbox_run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<sandbox_run::SandboxRun>,
    /// Which of the project's sandbox commands its sandbox runs, as checked
    /// when it was last started: by `repo_key`, then by the command's words
    /// (see `feature_sandbox`). What isn't here runs unless it is a comment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sandbox_picks: BTreeMap<String, BTreeMap<String, bool>>,
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
    /// The fix round open, if any (see `fix_round`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_round: Option<fix_round::FixRound>,
    /// The last round, once its fixes went and its agent rested: what it
    /// fixes can still be read and sent again, as resting doesn't tell the
    /// fixes were made. The next round starts afresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fix_round: Option<fix_round::FixRound>,
    /// Every round over or dropped, oldest first, as it last stood: the page
    /// of the stage that asked lists them, with what each changed
    /// (`fix_round::keep_past`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub past_rounds: Vec<fix_round::FixRound>,
    /// Discover's and Plan's drafts, each as it went (see `drafts`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<drafts::Draft>,
    /// How many fix rounds it had: the next one's number goes on from it.
    #[serde(default)]
    pub fix_rounds: u32,
    /// The fix round Manual test asked for, whose commits leave the marks
    /// as they are (see `keep_marks`): the user goes on trying the feature as
    /// they land. Forgotten once the code is read after the round is over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry: Option<u32>,
    /// The patch-id of the code those fixes made that Verify hasn't passed
    /// on yet: Manual test can't be marked passed until it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unverified: Option<String>,
    /// The stage whose fixes took the flow back to Verify (Manual test's,
    /// see `back_to_verify`): the rail draws the line back from it until
    /// Verify is marked done again, or the marks clear some other way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back_from: Option<Stage>,
    /// The machine that writes it, when that isn't this one: it is only shown
    /// here (see `view`). Never saved: the record is its writer's.
    #[serde(skip)]
    pub writer: Option<String>,
    /// How many times it moved to another machine's gerdr: of two records of it,
    /// the higher goes on (see `lease`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub epoch: u64,
    /// The machine it was last taken over from by a press here, by id: the one
    /// it can be handed back to (see `lease::HAND_BACK`). None once it was
    /// handed back, or never moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub came_from: Option<String>,
    /// The fix rounds Verify's failures started, oldest first: what Build's
    /// page says of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verify_fixes: Vec<fix_round::VerifyFix>,
    /// Which stage sent each fix round, by its number: what Build's page
    /// calls its list. Round numbers go on across a start over, so they stay.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fix_asked: BTreeMap<u32, Stage>,
    /// The fix rounds Code review's comments started, oldest first: what its
    /// page says of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comment_fixes: Vec<fix_round::CommentFix>,
    /// The last run number of each stage whose run was forgotten: a later
    /// run goes on from it, so its prompt doesn't overwrite an old one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub run_counts: BTreeMap<Stage, u32>,
}

/// What serde needs to leave out a number that says nothing.
fn is_zero(n: &u64) -> bool {
    *n == 0
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
    /// The last setup on machine `machine` (`None`: this one).
    pub fn setup_at(&self, machine: Option<&str>) -> Option<&setup::SetupRun> {
        (self.setup.iter().chain(&self.setups_elsewhere)).find(|s| s.machine.as_deref() == machine)
    }

    /// `run` is its setup now, the last it had elsewhere kept.
    pub fn begin_setup(&mut self, run: setup::SetupRun) {
        if let Some(old) = self.setup.replace(run)
            && old.machine != self.setup.as_ref().and_then(|s| s.machine.clone())
        {
            self.setups_elsewhere.push(old);
        }
        let machine = self.setup.as_ref().and_then(|s| s.machine.clone());
        self.setups_elsewhere.retain(|s| s.machine != machine);
    }

    /// Bring a record of an older format up to this one; whether it was
    /// older. A feature that went past Plan before Plan review existed has
    /// that stage marked done, at Plan's time, so it stays where it was.
    /// Nothing changes for 3: saving it moves its history out (see `store`).
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
        self.ran_once.remove(&stage);
        if stage >= Stage::Verify {
            self.back_from = None;
        }
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
        self.clear_marks_from(stage, now);
        if self.marks.len() == before {
            return false;
        }
        if self.status == Status::Done {
            self.status = Status::Open;
        }
        self.log(now, format!("{} to do again", stage.label()));
        true
    }

    /// Take away `stage`'s mark and every later one's, keeping when they were
    /// made in `ran_once`. The stage it was at ran too if it got anywhere, a
    /// mark or not: as of `now`.
    fn clear_marks_from(&mut self, stage: Stage, now: u64) {
        let was_at = self.current().filter(|at| {
            *at > stage
                && (self.runs.contains_key(at)
                    || self.has_output(*at, |_| false)
                    || self.fix_round.as_ref().is_some_and(|r| r.stage == *at))
        });
        let cleared = self.marks.split_off(&stage);
        self.back_from = None;
        self.ran_once
            .extend(cleared.into_iter().map(|(s, mark)| (s, mark.at)));
        self.ran_once.extend(was_at.map(|at| (at, now)));
        // Verify to pass again as a stage: no fixes wait for it apart.
        if stage <= Stage::Verify {
            self.unverified = None;
        }
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

    /// The code is now `patch`'s by the fixes Manual test asked for (`carry`),
    /// with Manual test current: the marks before it are moved onto it, and
    /// it is logged ("Code changed by fix round 2: marks kept"). Verify is
    /// to run on it again (`unverified`). Says whether they were; when not,
    /// `invalidate` says what goes.
    pub fn keep_marks(&mut self, patch: &str, now: u64) -> bool {
        let Some(n) = self.carry else {
            return false;
        };
        if self.status != Status::Open
            || self.current() != Some(Stage::ManualTest)
            || self.stale(patch).is_none()
        {
            return false;
        }
        for (stage, mark) in &mut self.marks {
            if stage.bound() && *stage < Stage::ManualTest {
                mark.patch = Some(patch.to_owned());
            }
        }
        self.log(now, format!("Code changed by fix round {n}: marks kept"));
        true
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
        self.clear_marks_from(stage, now);
        // The code went its own way: what Manual test fixed is gone over again.
        self.carry = None;
        self.unverified = None;
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
                Stage::PlanReview => self.plan_review.as_ref().is_some_and(|r| r.stamp.is_some()),
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
            // Never back: a parked run with a lower number can be forgotten
            // after a later one (see `feature_plan_review`).
            let counted = self.run_counts.entry(stage).or_default();
            *counted = run.n.max(*counted);
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
        // As if it had just been reached, nothing after it having run.
        self.ran_once.retain(|s, _| *s < stage);
        // What it fixed is gone with the stage that asked.
        if self
            .last_fix_round
            .as_ref()
            .is_some_and(|r| r.stage >= stage)
        {
            self.last_fix_round = None;
        }
        // Their runs' numbers start again with Verify's record.
        if stage <= Stage::Verify {
            self.verify_fixes.clear();
        }
        // The comments went with the review that made them.
        if stage <= Stage::CodeReview {
            self.comment_fixes.clear();
        }
        match stage {
            Stage::Discover | Stage::Plan | Stage::Build => {}
            Stage::PlanReview => self.plan_review = None,
            Stage::Verify => self.verify = None,
            Stage::AiReview => self.review = None,
            Stage::CodeReview => self.viewed = viewed::Viewed::default(),
            Stage::ManualTest => self.manual = None,
            Stage::Ship => {
                self.shipping = Shipping::default();
                self.ship_plan = ShipPlan::default();
            }
        }
        self.log(now, format!("{} started over", stage.label()));
        true
    }

    /// Open its last fix round again, unless one is open: on the stage it is
    /// at now if that comes after the one that asked. A later one keeps the
    /// round, as its page shows a round being fixed from an earlier stage, and
    /// the round's card stays on the page of the stage that asked.
    pub fn reopen_fix_round(&mut self) {
        if self.fix_round.is_some() {
            return;
        }
        let current = self.current();
        self.fix_round = self.last_fix_round.take().map(|mut round| {
            round.stage = current.map_or(round.stage, |at| at.max(round.stage));
            // Open again, it isn't over, and its diff goes on.
            round.ended = None;
            round.trail.to.clear();
            round
        });
        if current == Some(Stage::ManualTest) {
            self.carry = self.fix_round.as_ref().map(|r| r.n).or(self.carry);
        }
    }

    /// The open round's fixes go to the Build agent at `now`, the code as
    /// last read (`heads`, by `Repo::key`) if known: its trail keeps when the
    /// first went and where the code stood then, and the agent they go to.
    pub fn fixes_going(&mut self, now: u64, heads: Option<&BTreeMap<String, String>>) {
        let run = self.runs.get(&Stage::Build);
        let agent = run.map(|r| r.agent.clone()).unwrap_or_default();
        let model = run.and_then(|r| r.model.clone()).unwrap_or_default();
        let Some(round) = self.fix_round.as_mut() else {
            return;
        };
        let trail = &mut round.trail;
        trail.went.get_or_insert(now);
        if !agent.is_empty() {
            (trail.agent, trail.model) = (agent, model);
        }
        if trail.from.is_empty()
            && let Some(heads) = heads
        {
            trail.from = heads.clone();
        }
    }

    /// Its code reads `heads` (by `Repo::key`) at unix seconds `at`: the
    /// rounds' diffs take where they start, for one whose fixes went with no
    /// reading at hand (before any commit of theirs: the agent compacts
    /// first), and where they end, for those over since. Whether any changed.
    pub fn rounds_read(&mut self, heads: &BTreeMap<String, String>, at: u64) -> bool {
        let mut changed = false;
        if let Some(round) = self.fix_round.as_mut()
            && round.trail.from.is_empty()
            && !heads.is_empty()
            && round.trail.went.is_some_and(|went| at >= went)
            && (round.phase == fix_round::FixPhase::CompactingToFix || round.done == 0)
        {
            round.trail.from = heads.clone();
            changed = true;
        }
        let rounds = (self.last_fix_round.iter_mut()).chain(self.past_rounds.iter_mut());
        for round in rounds {
            changed |= round.code_read(heads, at);
        }
        changed
    }

    /// Every fix round it had that `stage`'s page lists, newest first: the
    /// open one, the last, then those before, each once as it last stood.
    pub fn rounds_of(&self, stage: Stage) -> Vec<&fix_round::FixRound> {
        let mut out: Vec<&fix_round::FixRound> = Vec::new();
        let all = (self.fix_round.iter())
            .chain(&self.last_fix_round)
            .chain(self.past_rounds.iter().rev());
        for round in all {
            let asked = self.fix_asked.get(&round.n).copied().unwrap_or(round.stage);
            if asked == stage && !out.iter().any(|r| r.n == round.n) {
                out.push(round);
            }
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.n));
        out
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

    /// `agent_dirs` for the agent `base` starts. Cursor's CLI also gets each
    /// repository's worktree under a folder that holds several: it tells an
    /// agent a root's git branch and status only for a root that is a git
    /// repository, which such a folder isn't, and a root's own search leaves
    /// out roots nested in it. A worktree that isn't there is left out, as the
    /// CLI refuses to start on a root that doesn't exist (checked on
    /// 2026.11.05).
    pub fn agent_dirs_for(&self, base: &str, data: &Path) -> Vec<PathBuf> {
        let mut dirs = self.agent_dirs(data);
        if agents::Flavor::of(base) == agents::Flavor::Cursor {
            let roots = self.repos.iter().map(|r| &r.worktree).filter(|worktree| {
                **worktree != self.dir && !dirs.contains(*worktree) && worktree.is_dir()
            });
            let roots: Vec<_> = roots.cloned().collect();
            dirs.extend(roots);
        }
        dirs
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
    fn a_run_number_is_never_used_twice() {
        let run = |n| agents::StageRun {
            n,
            ..agents::StageRun::default()
        };
        let mut f = done_up_to(Stage::Plan);
        // The reviewer, the writer, then the reviewer back and forgotten.
        f.runs.insert(Stage::PlanReview, run(2));
        f.forget_run(Stage::PlanReview);
        f.runs.insert(Stage::PlanReview, run(1));
        f.forget_run(Stage::PlanReview);
        assert_eq!(f.next_run(Stage::PlanReview), 3);
    }

    #[test]
    fn fix_rounds_keep_where_the_code_stood_when_their_fixes_went_and_ended() {
        let mut f = feature("x", 0);
        f.fix_round = Some(fix_round::FixRound {
            n: 1,
            what: "1 failure".into(),
            input: "/i".into(),
            stage: Stage::ManualTest,
            phase: fix_round::FixPhase::CompactingToFix,
            sent_back: 0,
            asked: 0,
            ready: None,
            stalled: None,
            give_back: fix_round::GiveBack::Nothing,
            items: Vec::new(),
            done: 0,
            ended: None,
            trail: Default::default(),
        });
        f.fix_asked.insert(1, Stage::ManualTest);
        let heads = |h: &str| BTreeMap::from([("api".to_owned(), h.to_owned())]);
        // No reading at hand: the next one gives where they start.
        f.fixes_going(100, None);
        assert!(f.fix_round.as_ref().unwrap().trail.from.is_empty());
        assert!(!f.rounds_read(&heads("a0"), 99));
        assert!(f.rounds_read(&heads("a1"), 101));
        // Sent again, it keeps where it started.
        f.fixes_going(150, Some(&heads("a9")));
        let trail = &f.fix_round.as_ref().unwrap().trail;
        assert_eq!((trail.went, trail.from.clone()), (Some(100), heads("a1")));
        // Over, the next reading gives where they end.
        let mut round = f.fix_round.take().unwrap();
        round.ended = Some(200);
        fix_round::keep_past(&mut f.past_rounds, round.clone());
        f.last_fix_round = Some(round);
        assert!(f.rounds_read(&heads("a2"), 201));
        assert_eq!(f.past_rounds[0].trail.to, heads("a2"));
        assert_eq!(f.rounds_of(Stage::ManualTest).len(), 1);
        assert!(f.rounds_of(Stage::CodeReview).is_empty());
        // Reopened, it goes on to HEAD again.
        f.reopen_fix_round();
        assert!(f.fix_round.as_ref().unwrap().trail.to.is_empty());
    }

    #[test]
    fn the_last_fix_round_opens_again_on_the_stage_it_is_at() {
        let round = |stage| fix_round::FixRound {
            n: 1,
            what: "1 finding".into(),
            input: "/d/a.md".into(),
            stage,
            phase: fix_round::FixPhase::Fixing,
            sent_back: 0,
            asked: 1,
            ready: None,
            stalled: None,
            give_back: Default::default(),
            items: Vec::new(),
            done: 0,
            ended: None,
            trail: Default::default(),
        };
        let mut f = feature("a", 1);
        let at = f.current().unwrap();
        f.last_fix_round = Some(round(Stage::Discover));
        f.reopen_fix_round();
        assert_eq!(f.fix_round.as_ref().map(|r| r.stage), Some(at));
        assert_eq!(f.last_fix_round, None);
        // A later stage that asked keeps it: its card stays on that page.
        let mut later = feature("a", 1);
        later.last_fix_round = Some(round(Stage::Ship));
        later.reopen_fix_round();
        assert_eq!(later.fix_round.as_ref().map(|r| r.stage), Some(Stage::Ship));
        // Open again, it isn't over.
        let mut over = feature("a", 1);
        over.last_fix_round = Some(fix_round::FixRound {
            ended: Some(5),
            ..round(Stage::Ship)
        });
        over.reopen_fix_round();
        assert_eq!(over.fix_round.as_ref().map(|r| r.ended), Some(None));
        // One open stays, and nothing is left to open.
        f.reopen_fix_round();
        assert!(f.fix_round.is_some());
        // Starting over the stage that asked, or one before, forgets it.
        let mut f = feature("b", 1);
        f.tick(1, "gerdr", None);
        let at = f.current().unwrap();
        f.last_fix_round = Some(round(at));
        assert!(f.start_over(at, 2));
        assert_eq!(f.last_fix_round, None);
        // One asked for earlier on stays.
        let mut f = feature("c", 1);
        f.tick(1, "gerdr", None);
        let at = f.current().unwrap();
        f.last_fix_round = Some(round(Stage::Discover));
        assert!(at > Stage::Discover);
        assert!(f.start_over(at, 2));
        assert!(f.last_fix_round.is_some());
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
            writer: None,
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
            ship_plan: Default::default(),
            runs: Default::default(),
            stage_agents: Default::default(),
            stage_machines: Default::default(),
            setups_elsewhere: Vec::new(),
            came_from: None,
            epoch: 0,
            stage_models: Default::default(),
            fresh_milestones: false,
            setup: None,
            checks: Default::default(),
            skip_checks: Default::default(),
            verify: None,
            sandbox: None,
            sandbox_picks: Default::default(),
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
            fix_round: None,
            last_fix_round: None,
            verify_fixes: Vec::new(),
            fix_asked: Default::default(),
            comment_fixes: Vec::new(),
            ran_once: Default::default(),
            fix_rounds: 0,
            past_rounds: Vec::new(),
            drafts: Vec::new(),
            carry: None,
            unverified: None,
            back_from: None,
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
            writer: None,
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
            ship_plan: Default::default(),
            run_counts: BTreeMap::new(),
            context: Vec::new(),
            removed: Vec::new(),
            runs: BTreeMap::new(),
            stage_agents: BTreeMap::new(),
            stage_machines: BTreeMap::new(),
            setups_elsewhere: Vec::new(),
            came_from: None,
            epoch: 0,
            stage_models: Default::default(),
            fresh_milestones: false,
            setup: None,
            checks: Default::default(),
            skip_checks: Default::default(),
            verify: None,
            sandbox: None,
            sandbox_picks: Default::default(),
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
            fix_round: None,
            last_fix_round: None,
            verify_fixes: Vec::new(),
            fix_asked: Default::default(),
            comment_fixes: Vec::new(),
            ran_once: Default::default(),
            fix_rounds: 0,
            past_rounds: Vec::new(),
            drafts: Vec::new(),
            carry: None,
            unverified: None,
            back_from: None,
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
    fn a_mark_that_goes_leaves_that_its_stage_ran_once() {
        let mut f = feature("a", 1);
        for at in 0..6 {
            f.tick(at, "you", Some("p1"));
        }
        assert!(f.ran_once.is_empty());
        // The code changes: Build on go, and when they were marked stays.
        assert_eq!(f.invalidate("p2", 9), Some(Stage::Build));
        assert_eq!(f.current(), Some(Stage::Build));
        let ran: Vec<_> = f.ran_once.iter().map(|(s, at)| (*s, *at)).collect();
        assert_eq!(
            ran,
            [(Stage::Build, 3), (Stage::Verify, 4), (Stage::AiReview, 5)]
        );
        // Marked again, it has its mark instead.
        f.tick(10, "you", Some("p2"));
        // The stage it was at had got somewhere, with no mark to go.
        f.tick(10, "you", Some("p2"));
        f.tick(10, "you", Some("p2"));
        assert_eq!(f.current(), Some(Stage::CodeReview));
        f.viewed.0.insert("api/a.rs".into(), "blob".into());
        assert_eq!(f.invalidate("p3", 10), Some(Stage::Build));
        assert_eq!(f.ran_once[&Stage::CodeReview], 10);
        f.tick(10, "you", Some("p3"));
        assert!(!f.ran_once.contains_key(&Stage::Build));
        assert!(f.ran_once.contains_key(&Stage::Verify));
        // Reopening leaves the same, and starting over forgets from there on.
        assert!(f.untick(Stage::Build, 11));
        assert_eq!(f.ran_once[&Stage::Build], 10);
        assert!(f.start_over(Stage::Build, 12));
        assert!(f.ran_once.is_empty());
    }

    #[test]
    fn manual_test_fixes_keep_the_marks() {
        let mut f = feature("a", 1);
        while f.current() != Some(Stage::ManualTest) {
            f.tick(1, "you", Some("p1"));
        }
        // No round of Manual test's: the code changing sends it back.
        assert!(!f.keep_marks("p2", 2));
        let mut kept = f.clone();
        assert_eq!(f.invalidate("p2", 2), Some(Stage::Build));
        // Its round's commits leave every mark, moved onto the new code.
        kept.carry = Some(3);
        assert!(!kept.keep_marks("p1", 2));
        assert!(kept.keep_marks("p2", 2));
        assert_eq!(kept.current(), Some(Stage::ManualTest));
        assert_eq!(kept.stale("p2"), None);
        assert_eq!(
            kept.history.last().unwrap().what,
            "Code changed by fix round 3: marks kept"
        );
        // Away from Manual test it doesn't hold.
        kept.untick(Stage::CodeReview, 3);
        assert!(!kept.keep_marks("p3", 3));
        assert_eq!(kept.invalidate("p3", 3), Some(Stage::Build));
        assert_eq!(kept.carry, None);
        // Verify done again as a stage, nothing waits on it apart.
        let mut f = feature("a", 1);
        while f.current() != Some(Stage::ManualTest) {
            f.tick(1, "you", Some("p1"));
        }
        f.unverified = Some("p3".into());
        f.manual = Some(manual::ManualTest::default());
        assert!(f.untick(Stage::Verify, 4));
        assert_eq!(f.unverified, None);
        // What Manual test found stays for when it is reached again.
        assert_eq!(f.current(), Some(Stage::Verify));
        assert!(f.manual.is_some());
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
    fn cursor_gets_each_repository_of_a_folder_of_several_as_a_root() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "web"], &["web"]);
        let data = Path::new("/data");
        let (api, web) = (f.dir.join("api"), f.dir.join("web"));
        assert_eq!(
            f.agent_dirs_for("cursor-agent --model x", data),
            [data.to_path_buf(), api.clone(), web.clone()],
            "an untouched repository too: its worktree is there to read"
        );
        assert_eq!(
            f.agent_dirs_for("env A=1 cursor-agent", data),
            [data.to_path_buf(), api.clone(), web.clone()]
        );
        for other in ["claude", "codex"] {
            assert_eq!(f.agent_dirs_for(other, data), [data.to_path_buf()]);
        }

        // The CLI refuses a root that doesn't exist.
        std::fs::remove_dir_all(&web).unwrap();
        assert_eq!(
            f.agent_dirs_for("cursor-agent", data),
            [data.to_path_buf(), api.clone()]
        );

        // The folder it starts in is its first root already.
        f.repos[0].worktree = f.dir.clone();
        f.repos.truncate(1);
        assert_eq!(f.agent_dirs_for("cursor-agent", data), [data.to_path_buf()]);
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
