//! The agent stages of features, run by gerdr: start a stage's agent on
//! its rendered prompt, watch it until it is done and its file is good
//! (nudging it once when it isn't), then wait at the gate for the user to
//! approve the file or send it back with comments (see `doc_review`).

use crate::{Gerdr, feature::Stage};

/// A gate waiting for the user: an agent stage's file is ready to review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    pub project: String,
    pub slug: String,
    /// The feature's title.
    pub title: String,
    pub stage: Stage,
    /// When it got ready (unix seconds).
    pub since: u64,
}

impl Gerdr {
    /// Start `stage`'s agent for feature `slug` (`fresh`: a new one even
    /// when its last run's pane is still there).
    pub fn start_stage_agent(&mut self, project: &str, slug: &str, stage: Stage, fresh: bool) {
        let _ = (project, slug, stage, fresh);
        todo!("feature_agents: start_stage_agent")
    }

    /// Look at every running stage agent: done with a good file makes its
    /// gate wait; done without one is nudged once. Called after each read
    /// of herdr's agents.
    pub fn stage_agents_tick(&mut self) {}

    /// The gates waiting for the user, oldest first, over every project.
    pub fn waiting_gates(&self) -> Vec<Gate> {
        Vec::new()
    }

    /// Approve `stage` of feature `slug`: its file as it is now. Marks
    /// the stage done.
    pub fn approve_stage(&mut self, project: &str, slug: &str, stage: Stage) {
        let _ = (project, slug, stage);
        todo!("feature_agents: approve_stage")
    }

    /// Send `stage` of feature `slug` back to its agent with `comments`
    /// (Markdown, as `review::markdown` writes it): pasted into its pane,
    /// else a fresh agent is started with them.
    pub fn send_back_stage(&mut self, project: &str, slug: &str, stage: Stage, comments: String) {
        let _ = (project, slug, stage, comments);
        todo!("feature_agents: send_back_stage")
    }
}
