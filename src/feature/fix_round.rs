//! A fix round: work sent back to a done Build agent goes through a plan the
//! user reviews first.
//!
//! Whatever asks for the fixes (Verify's failures, AI review's findings, Code
//! review's comments, Manual test's failures), the round goes the same way:
//! the agent's chat is compacted, the agent writes `fix-plan.md` and changes
//! no code, the user reads it, comments and sends it back as often as it
//! takes, and then makes the fixes. A feature has one round open at a time.
//!
//! This is the round's record and what decides its next step. Gerdr's side
//! is `crate::feature_fix`.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::{
    Stage, artifacts,
    review_import::{AiReview, FindingState},
};
use crate::herdr::AgentStatus;

/// The plan the agent writes, in the feature data dir.
pub const FILE: &str = "fix-plan.md";
/// The comments on it, in the feature data dir.
pub const COMMENTS: &str = "fix-plan-comments.json";
/// The section a plan must have, and not leave empty.
pub const FIXES: &str = "Fixes";
/// How long an agent told to compact may show no work: a chat too short to
/// compact stays idle, and the round must not hang on it. A compact that
/// was only not seen does no harm: the next line waits in the agent.
pub const COMPACT_STARTS: Duration = Duration::from_secs(15);
/// How long an agent told to plan may show no work before a file there
/// counts anyway: a written plan needn't wait for a status that was missed,
/// but one from before it was told again mustn't count at once.
pub const PLAN_STARTS: Duration = Duration::from_secs(15);

/// Where a round stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixPhase {
    /// The agent compacts its chat.
    #[default]
    Compacting,
    /// The agent writes the plan.
    Planning,
    /// The plan waits for the user.
    Ready,
    /// The agent makes the fixes.
    Fixing,
}

/// What a round took from where it came from, given back if it is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GiveBack {
    /// Nothing: Verify's failures stay on its run.
    #[default]
    Nothing,
    /// AI review's findings marked sent, by id, and the user's comments
    /// taken with them, as they were kept (`comments`).
    Findings {
        ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comments: Option<PathBuf>,
    },
    /// Code review's comments, as they were kept.
    Comments { copy: PathBuf },
    /// Manual test's failures marked sent, by criterion id and text.
    Manual { checks: Vec<(String, String)> },
}

/// An open round, kept in `feature.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixRound {
    /// Counts the feature's rounds, from 1.
    pub n: u32,
    /// What it is about, for the log, the toasts and the prompts: "2
    /// findings", "Verify's failures".
    pub what: String,
    /// The file saying what to fix.
    pub input: PathBuf,
    /// The stage that asked: its page shows the round.
    pub stage: Stage,
    #[serde(default)]
    pub phase: FixPhase,
    /// How many times the plan was sent back.
    #[serde(default)]
    pub sent_back: u32,
    /// Unix seconds it was last told to write the plan: an older file
    /// doesn't count.
    #[serde(default)]
    pub asked: u64,
    /// Unix seconds the plan came to wait for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<u64>,
    /// Why the agent isn't getting on with the plan, as said to the user
    /// once: it stopped without one, or its tab is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled: Option<String>,
    #[serde(default)]
    pub give_back: GiveBack,
}

impl FixRound {
    /// How the round stands, as a stage's card and rail say it: lowercase.
    pub fn line(&self) -> &'static str {
        match (self.phase, self.stalled.is_some()) {
            (FixPhase::Compacting, _) => "compacting the agent's chat",
            (FixPhase::Planning, true) => "no fix plan yet",
            (FixPhase::Planning, false) => "planning fixes",
            (FixPhase::Ready, _) => "fix plan ready",
            (FixPhase::Fixing, _) => "making fixes",
        }
    }

    /// Whether it still waits for the agent's plan or the user's word: the
    /// code isn't being changed for it yet.
    pub fn before_fixing(&self) -> bool {
        self.phase != FixPhase::Fixing
    }
}

/// Where the plan is, under feature data dir `dir`.
pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Where the comments on the plan are, under feature data dir `dir`.
pub fn comments_path(dir: &Path) -> PathBuf {
    dir.join(COMMENTS)
}

/// Where round `n`'s plan goes once the next round starts.
pub fn archive_path(dir: &Path, n: u32) -> PathBuf {
    dir.join("prompts").join(format!("fix-plan-{n}.md"))
}

/// What a plan's `text` lacks: nothing when it can be reviewed.
pub fn missing(text: &str) -> Vec<String> {
    let filled = artifacts::section(text, FIXES)
        .is_some_and(|lines| lines.iter().any(|line| !line.trim().is_empty()));
    match filled {
        true => Vec::new(),
        false => vec![format!("a filled `## {FIXES}` section")],
    }
}

/// AI review's findings `ids` go back to waiting to be sent.
pub fn findings_back(review: &mut AiReview, ids: &[String]) {
    for finding in &mut review.findings {
        if finding.state == FindingState::Sent && ids.contains(&finding.id) {
            finding.state = FindingState::Fix;
        }
    }
}

/// What a compacting agent comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compact {
    Wait,
    /// It works on it.
    Worked,
    /// Done, or it never will be: the plan's prompt goes.
    Go,
}

/// The next step of an agent told to compact: `worked` was seen working
/// since, `status` and `alive` as it is now, told `since` ago.
pub fn compact_next(
    worked: bool,
    status: Option<AgentStatus>,
    alive: bool,
    since: Duration,
) -> Compact {
    match status {
        _ if !alive => Compact::Go,
        Some(AgentStatus::Working | AgentStatus::Blocked) => Compact::Worked,
        _ if worked || since >= COMPACT_STARTS => Compact::Go,
        _ => Compact::Wait,
    }
}

/// What a planning agent comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Wait,
    /// It works on it.
    Worked,
    /// It rests with a new file there: read it.
    Read,
    /// It rests with no new file: it stopped without a plan.
    Stopped,
    /// It is gone.
    Gone,
}

/// The next step of an agent told to write the plan: `worked` was seen
/// working since, `status` and `alive` as it is now, told `since` ago,
/// `written` a file newer than that is there.
pub fn plan_next(
    worked: bool,
    status: Option<AgentStatus>,
    alive: bool,
    since: Duration,
    written: bool,
) -> Plan {
    match status {
        Some(AgentStatus::Working | AgentStatus::Blocked) if alive => Plan::Worked,
        _ if written && (worked || !alive || since >= PLAN_STARTS) => Plan::Read,
        _ if !alive => Plan::Gone,
        _ if worked => Plan::Stopped,
        _ => Plan::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOON: Duration = Duration::from_secs(1);
    const LATE: Duration = Duration::from_secs(60);

    #[test]
    fn a_compact_goes_on_once_done_or_if_nothing_comes_of_it() {
        let idle = Some(AgentStatus::Idle);
        let working = Some(AgentStatus::Working);
        assert_eq!(compact_next(false, idle, true, SOON), Compact::Wait);
        assert_eq!(compact_next(false, working, true, SOON), Compact::Worked);
        assert_eq!(compact_next(true, working, true, LATE), Compact::Worked);
        assert_eq!(compact_next(true, idle, true, SOON), Compact::Go);
        // "Not enough messages to compact": it never works, and goes on.
        assert_eq!(compact_next(false, idle, true, LATE), Compact::Go);
        assert_eq!(compact_next(false, None, false, SOON), Compact::Go);
    }

    #[test]
    fn a_plan_counts_once_the_agent_rests_with_a_new_file() {
        let idle = Some(AgentStatus::Idle);
        let working = Some(AgentStatus::Working);
        let blocked = Some(AgentStatus::Blocked);
        assert_eq!(plan_next(false, idle, true, SOON, false), Plan::Wait);
        assert_eq!(plan_next(false, working, true, SOON, false), Plan::Worked);
        // Still writing it, or asking something.
        assert_eq!(plan_next(true, working, true, SOON, true), Plan::Worked);
        assert_eq!(plan_next(true, blocked, true, SOON, true), Plan::Worked);
        assert_eq!(plan_next(true, idle, true, SOON, true), Plan::Read);
        assert_eq!(plan_next(true, idle, true, SOON, false), Plan::Stopped);
        // A file just written, the agent not seen working yet: not at once.
        assert_eq!(plan_next(false, idle, true, SOON, true), Plan::Wait);
        assert_eq!(plan_next(false, idle, true, LATE, true), Plan::Read);
        assert_eq!(plan_next(true, None, false, SOON, false), Plan::Gone);
        assert_eq!(plan_next(false, None, false, SOON, true), Plan::Read);
    }

    #[test]
    fn a_plan_needs_its_fixes_section_filled() {
        assert!(missing("## Fixes\n- Stop at n in src/a.rs\n").is_empty());
        // One subheading a fix, with no line of its own above them.
        assert!(missing("## Fixes\n\n### r1-1: Drops it\n\nCause.\n").is_empty());
        assert_eq!(missing("# Plan\n\nNothing.\n").len(), 1);
        assert_eq!(missing("## Fixes\n\n## Questions\n- Why?\n").len(), 1);
    }

    #[test]
    fn a_round_written_before_a_field_came_still_reads() {
        let round: FixRound = serde_json::from_str(
            r#"{"n":2,"what":"2 findings","input":"/d/a.md","stage":"verify"}"#,
        )
        .unwrap();
        assert_eq!(round.phase, FixPhase::Compacting);
        assert_eq!(round.give_back, GiveBack::Nothing);
        assert_eq!(round.line(), "compacting the agent's chat");
    }
}
