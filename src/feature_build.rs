//! The Build and Verify stages of features, run by gerdr.
//!
//! Setup: once a feature's worktrees are made, the store file's setup
//! commands run off the thread (see `feature::setup`), shown in the tasks
//! panel. Starting the Build agent while the setup failed or never ran
//! runs it first, and starts the agent once it is done.
//!
//! Build: its agent (see `feature_agents`) builds the plan's milestones
//! one at a time in the touched worktrees, commits each, ticks it in
//! `build.md`, and stops: the feature waits at a milestone gate until the
//! user lets it go on (`continue_stage_agent`). With every milestone
//! ticked and the agent done, gerdr reads the code: nothing uncommitted,
//! no change in an untouched repository, at least one commit. Then gerdr
//! marks Build done on that code and runs Verify.
//!
//! Verify: the store file's checks (see `feature::verify`) run off the
//! thread; `evidence.md` is written either way. Passing marks Verify done
//! on the code the checks ran on. Failing sends the failures to the Build
//! agent, in its tab while it lives, else to a fresh one, up to
//! `verify::MAX_FIXES` times in a row; its fix commits send the feature
//! back to Build (the patch watch), and the loop goes round. A project
//! without checks keeps Verify a manual mark.

use crate::{
    Gerdr,
    feature::{Stage, patch::Reading, verify::Evidence},
    feature_agents::Stamp,
};

/// What Build and Verify keep while gerdr runs; not saved.
#[derive(Debug, Default)]
pub(crate) struct BuildState {}

impl Gerdr {
    /// The code of feature `slug` was read for run `n` of its Build agent,
    /// whose `build.md` had `stamp`: Build is marked done on it if it
    /// holds, and Verify runs; else the agent is told what is missing.
    pub(crate) fn build_code_checked(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: Stamp,
        result: Result<Reading, String>,
    ) {
        let _ = (project, slug, n, stamp, result);
    }

    /// Set up feature `slug`'s worktrees off the thread, if its project
    /// gives setup commands and it isn't set up or being set up.
    pub fn start_setup(&mut self, project: &str, slug: &str) {
        let _ = (project, slug);
    }

    /// The setup of feature `slug` ended: kept, and a Build agent waiting
    /// on it starts.
    pub(crate) fn setup_done(&mut self, project: &str, slug: &str, result: Result<(), String>) {
        let _ = (project, slug, result);
    }

    /// Run Verify's checks on feature `slug` off the thread.
    pub fn run_verify(&mut self, project: &str, slug: &str) {
        let _ = (project, slug);
    }

    /// Run `n` of Verify's checks of feature `slug` came back with
    /// `evidence`: written, then marked done, or the failures sent to the
    /// Build agent.
    pub(crate) fn verify_done(&mut self, project: &str, slug: &str, n: u32, evidence: Evidence) {
        let _ = (project, slug, n, evidence);
    }

    /// Let `stage`'s agent of feature `slug` go on after a milestone stop.
    pub fn continue_stage_agent(&mut self, project: &str, slug: &str, stage: Stage) {
        let _ = (project, slug, stage);
    }
}
