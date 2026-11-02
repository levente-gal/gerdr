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
//! In this first increment every stage is a manual tick: the user marks it
//! done in the feature panel and gerdr records when. Later increments run
//! the stages' agents and checks, and fill the ticks themselves; the record
//! keeps its shape.

#![allow(dead_code)]

pub mod layout;
pub mod store;

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

    /// Mark the current stage done at `now`, by `by`; when it is Ship, the
    /// feature is done. Only the current stage can be ticked: stages go in
    /// order. Returns the stage ticked, or `None` when there is none.
    pub fn tick(&mut self, now: u64, by: &str) -> Option<Stage> {
        let _ = (now, by);
        None
    }

    /// Clear `stage`'s mark and every later one's, as when it has to be
    /// done again. A done feature opens again. Returns whether anything
    /// was cleared.
    pub fn untick(&mut self, stage: Stage, now: u64) -> bool {
        let _ = (stage, now);
        false
    }

    /// Add `what` to its history at `now`.
    pub fn log(&mut self, now: u64, what: impl Into<String>) {
        let _ = (now, what.into());
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
