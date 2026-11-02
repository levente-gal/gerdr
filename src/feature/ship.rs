//! Shipping a feature, and what stops it.
//!
//! `probe` reads a feature for the panel, off the UI thread: its patch-id,
//! its guards, and when Ship is its stage, what blocks shipping or else
//! what shipping does. Shipping then goes one of two ways, per repository
//! (`ShipMode`):
//!
//! - pull requests (`open_prs`): for each touched repository with commits,
//!   in name order, allow the push (`hook::allow`), push `feature/<slug>`
//!   with its upstream, guard it again, and open a pull request into its
//!   base with `gh pr create`, or keep the one open for the branch already.
//!   With more than one, every body links the others (`gh pr edit`).
//! - squash (`squash_command`): a command for a tab in the main worktree
//!   that squashes the branch into the base and commits, the message
//!   prefilled with the title, for the user to edit. The feature is done
//!   once every branch reads as merged (`Probe::merged`).
//!
//! Runs git and gh: call it off the UI thread (checklist D3).

use std::{collections::BTreeMap, path::PathBuf};

use super::{Feature, Pr, ShipMode, hook::Guard, patch::Reading};

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
}

impl Action {
    /// In words, for the Ship sheet: "Push services-api", "Open a pull
    /// request into dev for services-api".
    pub fn label(&self) -> String {
        format!("{self:?}")
    }
}

/// Whether a feature can ship now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipCheck {
    /// What stops it, one line each ("services-api has uncommitted
    /// changes"); empty when it can ship.
    pub blockers: Vec<String>,
    /// What shipping does, in order; empty while blocked.
    pub actions: Vec<Action>,
    /// What is worth knowing but stops nothing ("aft-backend has no
    /// commits: left out").
    pub notes: Vec<String>,
}

/// What `probe` read of a feature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// Its patch-id and uncommitted changes, or why they couldn't be read.
    pub reading: Result<Reading, String>,
    /// Per touched repository, by name: whether pushes from its worktree
    /// are guarded (after making sure of the hook there).
    pub guards: BTreeMap<String, Guard>,
    /// When Ship is its current stage: whether it can ship.
    pub ship: Option<ShipCheck>,
    /// When a squash started (`Shipping::squash_at`): whether every
    /// touched repository with commits reads as merged into its base.
    pub merged: bool,
}

/// How `repo` of `feature` ships: `mode` when the project sets one, else
/// a pull request when its `origin` is on GitHub, else a squash.
pub fn mode_of(feature: &Feature, repo: &str, mode: Option<ShipMode>) -> ShipMode {
    let _ = (feature, repo);
    mode.unwrap_or(ShipMode::Squash)
}

/// Read `feature` for the panel (see `Probe`); `mode` is the project's
/// setting, if any.
pub fn probe(feature: &Feature, mode: Option<ShipMode>) -> Probe {
    let _ = (feature, mode);
    Probe {
        reading: Err("not built yet".into()),
        guards: BTreeMap::new(),
        ship: None,
        merged: false,
    }
}

/// Run `actions` of a pull-request ship of `feature`, with `gh` the
/// program to run as gh. Returns the pull requests, or why it stopped with
/// those opened by then: running it again goes on from there.
pub fn open_prs(
    feature: &Feature,
    actions: &[Action],
    gh: &str,
) -> Result<Vec<Pr>, (String, Vec<Pr>)> {
    let _ = (feature, actions, gh);
    Err(("not built yet".into(), Vec::new()))
}

/// The folder and the shell command of a squash ship of `feature`'s
/// `actions`, for a tab: every `Squash` action's repository in turn.
pub fn squash_command(feature: &Feature, actions: &[Action]) -> (PathBuf, String) {
    let _ = actions;
    (feature.root.clone(), String::new())
}
