//! The one way lines reach a feature's stage agents: its prompt, comments
//! sent back, nudges, fix rounds. Each goes through `tell_stage_agent`, so
//! what was typed can be kept with the feature's AI work (see `ai_work`).

use crate::{
    Gerdr, agents::AgentKey, ai_work::SendKind, feature::Stage, feature_build::TAB_GONE,
    machine_work::split_pane,
};

impl Gerdr {
    /// Type `line` into the agent of `stage` of feature `slug`, for `kind`.
    /// It goes as it does today: a nudge straight to where the agent is; a
    /// prompt to its tab while it waits for one; anything else only once
    /// the prompt was sent, and on Build to its T3 Code thread too.
    #[allow(dead_code)] // its callers move to it in wave 1 (AI work contract)
    pub(crate) fn tell_stage_agent(
        &mut self,
        project: &str,
        slug: &str,
        stage: Stage,
        kind: SendKind,
        line: &str,
    ) -> Result<(), String> {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return Err("the feature is gone".into());
        };
        let run = feature.runs.get(&stage);
        if kind == SendKind::Nudge {
            return match run.and_then(|r| r.agent_key()) {
                Some(AgentKey::T3(id)) => self.t3_send(&id, line),
                Some(AgentKey::Herdr(pane)) if split_pane(&pane).is_some() => {
                    self.tell_machine_pane(&pane, line)
                }
                Some(AgentKey::Herdr(pane)) => match self.herdr.as_mut() {
                    Some(herdr) => herdr.send_agent_line(&pane, line),
                    None => Err("herdr isn't connected".into()),
                },
                _ => Err(TAB_GONE.into()),
            };
        }
        if stage == Stage::Build && kind != SendKind::Start {
            return self.tell_build_agent(&feature, line);
        }
        if kind != SendKind::Start && run.is_some_and(|r| r.held) {
            return Err("its prompt wasn't sent yet".into());
        }
        let pane = run
            .and_then(|r| r.pane.clone())
            .filter(|p| self.pane_there(p))
            .ok_or_else(|| String::from(TAB_GONE))?;
        self.tell_agent_pane(&pane, line)
    }
}
