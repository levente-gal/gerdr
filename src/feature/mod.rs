//! Features: a unit of work gerdr takes through eight stages, from a chat
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
pub mod hook;
pub mod layout;
pub mod manual;
pub mod patch;
pub mod review_import;
pub mod setup;
pub mod ship;
pub mod store;
pub mod verify;
pub mod viewed;

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// The record's format; bumped when a field changes meaning.
pub const VERSION: u32 = 1;

/// The stages, in order. Every one is mandatory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Discover,
    Plan,
    Build,
    Verify,
    AiReview,
    CodeReview,
    ManualTest,
    Ship,
}

impl Stage {
    pub const ALL: [Self; 8] = [
        Self::Discover,
        Self::Plan,
        Self::Build,
        Self::Verify,
        Self::AiReview,
        Self::CodeReview,
        Self::ManualTest,
        Self::Ship,
    ];

    /// Its name in the panel: "AI review".
    pub fn label(self) -> &'static str {
        match self {
            Self::Discover => "Discover",
            Self::Plan => "Plan",
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
    /// The repository's name, as in `Repo::name`.
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
    /// Whether the feature changes it: then its worktree is on `branch`,
    /// else detached at `base_commit`.
    pub touched: bool,
    /// `feature/<slug>` once touched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
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
    /// Every repository, touched or not, in name order.
    pub repos: Vec<Repo>,
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
    /// Code review's viewed ticks (see `viewed`).
    #[serde(default, skip_serializing_if = "viewed::Viewed::is_empty")]
    pub viewed: viewed::Viewed,
    /// Manual test's results (see `manual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<manual::ManualTest>,
}

impl Feature {
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
            status: Status::Open,
            marks: Default::default(),
            history: Vec::new(),
            shipping: Default::default(),
            runs: Default::default(),
            setup: None,
            verify: None,
            review: None,
            viewed: Default::default(),
            manual: None,
        }
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
            status: Status::Open,
            marks: BTreeMap::new(),
            history: Vec::new(),
            shipping: Shipping::default(),
            runs: BTreeMap::new(),
            setup: None,
            verify: None,
            review: None,
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
        assert_eq!(f.current(), Some(Stage::Build));
        assert_eq!(f.marks[&Stage::Plan].at, 11);
        assert_eq!(f.marks[&Stage::Plan].by, "you");
        assert_eq!(f.history.last().unwrap().what, "Plan ticked");
        for _ in 2..7 {
            f.tick(12, "you", None).unwrap();
            assert_eq!(f.status, Status::Open);
        }
        assert_eq!(f.tick(13, "you", None), Some(Stage::Ship));
        assert_eq!(f.status, Status::Done);
        assert_eq!(f.tick(14, "you", None), None);
        assert_eq!(f.history.len(), 8);
    }

    #[test]
    fn untick_clears_later_stages_and_reopens() {
        let mut f = feature("a", 1);
        for _ in 0..8 {
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
        // Discover and Plan keep no patch: they hold whatever the code.
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
        for _ in 0..8 {
            f.tick(1, "you", Some("p1"));
        }
        assert_eq!(f.status, Status::Done);
        assert_eq!(f.invalidate("p2", 2), None);
        assert_eq!(f.current(), None);
        f.status = Status::Abandoned;
        assert_eq!(f.invalidate("p2", 2), None);
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
}
