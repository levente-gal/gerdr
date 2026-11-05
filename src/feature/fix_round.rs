//! A fix round: work sent back to a done Build agent goes through a plan the
//! user reviews first.
//!
//! Whatever asks for the fixes (Verify's failures, AI review's findings, Code
//! review's comments, Manual test's failures), the round goes the same way:
//! the agent's chat is compacted, the agent writes `fix-plan.md` and changes
//! no code, the user reads it, comments and sends it back as often as it
//! takes, and then makes the fixes, the chat compacted again first as the
//! plan is in its file. A feature has one round open at a time.
//!
//! The plan must speak of everything the round was sent. gerdr lists its
//! fixes in the build log, apart from the milestones, and the round is over
//! once every one is ticked: an agent resting before that is told to go on,
//! and one that won't is the user's to send again or drop.
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
/// How a round stands once every fix of it is ticked in the build log.
pub const SENT: &str = "fixes made";
/// How long an agent told to compact may show no work: a chat too short to
/// compact stays idle, and the round must not hang on it. A compact that
/// was only not seen does no harm: the next line waits in the agent.
pub const COMPACT_STARTS: Duration = Duration::from_secs(15);
/// How long an agent told to plan may show no work before a file there
/// counts anyway: a written plan needn't wait for a status that was missed,
/// but one from before it was told again mustn't count at once.
pub const PLAN_STARTS: Duration = Duration::from_secs(15);
/// How long a fixing agent must show no work and leave the build log alone
/// to count as resting: herdr can show one idle for a moment between two
/// steps, and a line typed then lands in the middle of its work.
pub const REST: Duration = Duration::from_secs(30);
/// The same when herdr can't say what the agent does.
pub const QUIET: Duration = Duration::from_secs(180);
/// How many times an agent resting with no more fixes ticked is told to go
/// on before the user is asked.
pub const GO_ONS: u32 = 1;
/// What stands for a plan's fixes that it doesn't list one by one.
pub const ALL_FIXES: &str = "All the fixes of the plan";

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
    /// The user said to make the fixes: the agent compacts its chat first.
    CompactingToFix,
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
    /// What the agent must tick in the build log for the round to end, once
    /// the fixes went.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    /// How many of them were ticked when last read.
    #[serde(default)]
    pub done: u32,
}

impl FixRound {
    /// How the round stands, as a stage's card and rail say it: lowercase.
    pub fn line(&self) -> String {
        let total = self.items.len();
        match (self.phase, self.stalled.is_some()) {
            (FixPhase::Compacting | FixPhase::CompactingToFix, _) => {
                "compacting the agent's chat".into()
            }
            (FixPhase::Planning, true) => "no fix plan yet".into(),
            (FixPhase::Planning, false) => "planning fixes".into(),
            (FixPhase::Ready, _) => "fix plan ready".into(),
            (FixPhase::Fixing, true) => {
                format!("fixes stopped \u{b7} {} of {total} made", self.done)
            }
            (FixPhase::Fixing, false) if total == 0 => "making fixes".into(),
            (FixPhase::Fixing, false) => {
                format!("making fixes \u{b7} {} of {total} made", self.done)
            }
        }
    }

    /// Whether it still waits for the agent's plan or the user's word: the
    /// code isn't being changed for it yet.
    pub fn before_fixing(&self) -> bool {
        !matches!(self.phase, FixPhase::CompactingToFix | FixPhase::Fixing)
    }

    /// Whether its agent stopped with fixes left: the user's turn.
    pub fn fixes_stopped(&self) -> bool {
        self.phase == FixPhase::Fixing && self.stalled.is_some()
    }

    /// Whether the user can tell the agent to make the fixes now.
    pub fn can_make_fixes(&self) -> bool {
        self.phase == FixPhase::Ready || self.fixes_stopped()
    }

    /// Whether the user can drop it now: not with the agent making fixes.
    pub fn can_drop(&self) -> bool {
        self.before_fixing() || self.fixes_stopped()
    }

    /// The ids of what it was asked to fix, where those have ids: its plan
    /// must speak of each.
    pub fn ids(&self) -> Vec<String> {
        match &self.give_back {
            GiveBack::Findings { ids, .. } => ids.clone(),
            GiveBack::Manual { checks } => checks.iter().map(|(id, _)| id.clone()).collect(),
            GiveBack::Nothing | GiveBack::Comments { .. } => Vec::new(),
        }
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

/// Whether `text` names `id` as a whole word: `r1-1` isn't in `r1-11`.
fn names(text: &str, id: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .any(|word| word == id)
}

/// What a plan's `text` lacks: nothing when it can be reviewed. It must
/// speak of each of `ids`, what the round was asked to fix.
pub fn missing(text: &str, ids: &[String]) -> Vec<String> {
    let Some(lines) = artifacts::section(text, FIXES).filter(|l| !l.is_empty()) else {
        return vec![format!("a filled `## {FIXES}` section")];
    };
    let left: Vec<&str> = (ids.iter())
        .filter(|id| !lines.iter().any(|line| names(line, id)))
        .map(String::as_str)
        .collect();
    match left.is_empty() {
        true => Vec::new(),
        false => vec![format!("a fix for {}", left.join(", "))],
    }
}

/// What the agent must tick for a plan reading `text`: each heading under
/// its `## Fixes` at the top level found there, and then each of `ids` no
/// heading names. A plan with neither has one item, standing for all of it.
pub fn items(text: &str, ids: &[String]) -> Vec<String> {
    let lines = artifacts::section(text, FIXES).unwrap_or_default();
    let level = |line: &str| line.len() - line.trim_start_matches('#').len();
    let is_heading = |line: &&&str| {
        let n = level(line);
        n > 0 && line[n..].starts_with(' ')
    };
    let top = lines.iter().filter(is_heading).map(|l| level(l)).min();
    let mut items: Vec<String> = (lines.iter())
        .filter(is_heading)
        .filter(|line| Some(level(line)) == top)
        .map(|line| line.trim_start_matches('#').trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect();
    let bare: Vec<String> = (ids.iter())
        .filter(|id| !items.iter().any(|item| names(item, id)))
        .cloned()
        .collect();
    items.extend(bare);
    if items.is_empty() {
        items.push(ALL_FIXES.to_owned());
    }
    items
}

/// The build log's heading for round `n`'s fixes.
pub fn heading(n: u32) -> String {
    format!("Fix round {n}")
}

/// Whether `line`, what a box in the log says, is `item`: an agent may add
/// to it, as long as it keeps what gerdr wrote.
fn is_item(line: &str, item: &str) -> bool {
    line.starts_with(item)
}

/// Build log `log` with round `n`'s section holding a box for each of
/// `items`: those there already stay as they are, ticked or not.
pub fn with_items(log: &str, n: u32, items: &[String]) -> String {
    let heading = heading(n);
    let there = artifacts::items_under(log, &heading);
    let new: Vec<String> = (items.iter())
        .filter(|item| !there.iter().any(|(_, line)| is_item(line, item)))
        .map(|item| format!("- [ ] {item}"))
        .collect();
    if new.is_empty() {
        return log.to_owned();
    }
    let own = format!("## {heading}");
    let lines: Vec<&str> = log.lines().collect();
    let Some(start) = lines.iter().position(|line| line.trim() == own) else {
        let log = log.trim_end();
        let gap = if log.is_empty() { "" } else { "\n\n" };
        return format!("{log}{gap}{own}\n\n{}\n", new.join("\n"));
    };
    // After what the section holds, before the next section.
    let next = (lines[start + 1..].iter())
        .position(|line| line.starts_with("# ") || line.starts_with("## "))
        .map_or(lines.len(), |i| start + 1 + i);
    let end = (start + 1..next)
        .rev()
        .find(|i| !lines[*i].trim().is_empty())
        .map_or(start + 1, |i| i + 1);
    let mut out: Vec<&str> = lines[..end].to_vec();
    if end == start + 1 {
        out.push("");
    }
    out.extend(new.iter().map(String::as_str));
    out.extend_from_slice(&lines[end..]);
    format!("{}\n", out.join("\n"))
}

/// Which of `items` build log `log` doesn't tick for round `n`.
pub fn left(log: &str, n: u32, items: &[String]) -> Vec<String> {
    let there = artifacts::items_under(log, &heading(n));
    (items.iter())
        .filter(|item| {
            !there
                .iter()
                .any(|(ticked, line)| *ticked && is_item(line, item))
        })
        .cloned()
        .collect()
}

/// Build log `log` without the unticked boxes its agent added under
/// `## Milestones` past the plan's `planned`: the round's own section has
/// the fixes, and those would hold Build up as milestones never to be built.
pub fn without_added_milestones(log: &str, planned: usize) -> String {
    if planned == 0 {
        return log.to_owned();
    }
    let mut out = Vec::new();
    let (mut inside, mut boxes) = (false, 0);
    for line in log.lines() {
        if line.starts_with("# ") || line.starts_with("## ") {
            inside = line
                .trim_start_matches('#')
                .trim()
                .eq_ignore_ascii_case("Milestones");
        }
        if inside && let Some(mark) = artifacts::checkbox(line.trim_start()) {
            boxes += 1;
            if boxes > planned && mark == "[ ]" {
                continue;
            }
        }
        out.push(line);
    }
    format!("{}\n", out.join("\n"))
}

/// What a fixing agent comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixes {
    Wait,
    /// It works on them.
    Worked,
    /// It rests with every fix ticked: the round is over.
    Done,
    /// It rests with fixes left, and more were ticked since it was last
    /// told: tell it to go on.
    GoOn,
    /// It rests with fixes left, as it says, because of the plan.
    Blocked,
    /// It rests with fixes left, told to go on already: the user's turn.
    Stopped,
    /// It is gone with fixes left.
    Gone,
}

/// The next step of an agent making a round's fixes: `status` and `alive`
/// as it is now, `rested` since it last worked or touched the build log,
/// `left` fixes not ticked, `blocked` the log says so, `told` times to go
/// on with no more ticked since.
pub fn fixes_next(
    status: Option<AgentStatus>,
    alive: bool,
    rested: Duration,
    left: usize,
    blocked: bool,
    told: u32,
) -> Fixes {
    let rest = match status {
        Some(AgentStatus::Working | AgentStatus::Blocked) if alive => return Fixes::Worked,
        Some(AgentStatus::Idle | AgentStatus::Done) => REST,
        _ if !alive => REST,
        _ => QUIET,
    };
    match () {
        () if rested < rest => Fixes::Wait,
        () if left == 0 => Fixes::Done,
        () if !alive => Fixes::Gone,
        () if blocked => Fixes::Blocked,
        () if told >= GO_ONS => Fixes::Stopped,
        () => Fixes::GoOn,
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

    #[test]
    fn a_compact_before_the_fixes_is_past_the_users_word() {
        let round = |phase| FixRound {
            n: 1,
            what: "1 finding".into(),
            input: "/d/a.md".into(),
            stage: Stage::Verify,
            phase,
            sent_back: 0,
            asked: 1,
            ready: None,
            stalled: None,
            give_back: GiveBack::Nothing,
            items: Vec::new(),
            done: 0,
        };
        let compacting = round(FixPhase::CompactingToFix);
        assert!(!compacting.before_fixing());
        assert_eq!(compacting.line(), "compacting the agent's chat");
        assert!(round(FixPhase::Compacting).before_fixing());
        // Kept in `feature.json`: a restart goes on from it.
        let kept = serde_json::to_string(&compacting).unwrap();
        let back: FixRound = serde_json::from_str(&kept).unwrap();
        assert_eq!(back.phase, FixPhase::CompactingToFix);
    }

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
    fn a_plan_must_speak_of_everything_it_was_asked_to_fix() {
        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let plan = "## Fixes\n\n### r1-11: Off by one\n\nChange a.\n\n### r1-2\n\nAlso r1-3.\n\n## Questions\n\nr1-4?\n";
        assert!(missing(plan, &ids(&["r1-11", "r1-2", "r1-3"])).is_empty());
        // Not a part of another's id, nor elsewhere in the file.
        assert_eq!(
            missing(plan, &ids(&["r1-1", "r1-4"])),
            vec!["a fix for r1-1, r1-4".to_owned()]
        );
    }

    #[test]
    fn a_plans_items_are_its_headings_and_what_they_leave_out() {
        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let plan = "## Fixes\n\nEach below.\n\n### r1-1 and r1-2\n\n#### Why\n\n### Review comment\n\nAlso r1-3.\n\n## Questions\n\n### Not this\n";
        assert_eq!(
            items(plan, &ids(&["r1-1", "r1-2", "r1-3"])),
            ["r1-1 and r1-2", "Review comment", "r1-3"]
        );
        // Nothing to go by: one box stands for it all.
        assert_eq!(items("## Fixes\n\n- a\n", &[]), [ALL_FIXES]);
    }

    #[test]
    fn items_go_in_their_rounds_section_once_and_keep_their_ticks() {
        let items: Vec<String> = vec!["r1-1: a".into(), "r1-2: b".into()];
        let log = "# T\n\n## Milestones\n\n- [x] One\n";
        let with = with_items(log, 1, &items);
        assert_eq!(
            with,
            "# T\n\n## Milestones\n\n- [x] One\n\n## Fix round 1\n\n- [ ] r1-1: a\n- [ ] r1-2: b\n"
        );
        // They aren't milestones: Build's count is as it was.
        assert_eq!(artifacts::milestones(&with), (1, 1));
        assert_eq!(left(&with, 1, &items), items);
        // Ticked with a note, it stays so when they are written again.
        let ticked = with.replace("- [ ] r1-1: a", "- [x] r1-1: a, commit abc\n\n  Note.");
        assert_eq!(with_items(&ticked, 1, &items), ticked);
        assert_eq!(left(&ticked, 1, &items), ["r1-2: b"]);
        // One more goes after them, before the next section.
        let more: Vec<String> = vec!["r1-1: a".into(), "r1-2: b".into(), "r1-3: c".into()];
        let later = format!("{ticked}\n## Notes\n\nHi\n");
        let grown = with_items(&later, 1, &more);
        assert!(
            grown.contains("- [ ] r1-2: b\n- [ ] r1-3: c\n\n## Notes"),
            "{grown}"
        );
        // Another round has its own.
        assert_eq!(left(&ticked, 2, &items), items);
        // A box gone counts as not done, and so does a box of another text.
        assert_eq!(left("## Fix round 1\n\n- [x] other\n", 1, &items), items);
    }

    #[test]
    fn milestones_an_agent_added_go_once_past_the_plans() {
        let log = "## Milestones\n\n- [x] One\n\n  Note.\n\n- [x] r1-1\n\n- [ ] r1-2\n\n## Fix round 1\n\n- [ ] r1-9\n";
        let tidy = without_added_milestones(log, 1);
        assert_eq!(artifacts::milestones(&tidy), (2, 2));
        assert!(tidy.contains("- [ ] r1-9"), "{tidy}");
        // A plan's own unticked milestone stays, and an unread plan leaves all.
        assert_eq!(
            artifacts::milestones(&without_added_milestones(log, 3)),
            (2, 3)
        );
        assert_eq!(without_added_milestones(log, 0), log);
    }

    #[test]
    fn a_fixing_agent_is_told_to_go_on_until_all_is_ticked() {
        use AgentStatus::{Idle, Working};
        let long = REST + Duration::from_secs(1);
        let next = |status, alive, rested, left, blocked, told| {
            fixes_next(status, alive, rested, left, blocked, told)
        };
        assert_eq!(next(Some(Working), true, long, 3, false, 0), Fixes::Worked);
        // Idle for a moment between two steps is no rest.
        assert_eq!(
            next(Some(Idle), true, Duration::ZERO, 3, false, 0),
            Fixes::Wait
        );
        assert_eq!(next(Some(Idle), true, long, 0, false, 0), Fixes::Done);
        assert_eq!(next(Some(Idle), true, long, 3, false, 0), Fixes::GoOn);
        // Told once with nothing more ticked: the user's turn.
        assert_eq!(
            next(Some(Idle), true, long, 3, false, GO_ONS),
            Fixes::Stopped
        );
        assert_eq!(next(Some(Idle), true, long, 3, true, 0), Fixes::Blocked);
        assert_eq!(next(None, false, long, 3, false, 0), Fixes::Gone);
        assert_eq!(next(None, false, long, 0, false, 0), Fixes::Done);
        // herdr can't say: only a long quiet counts.
        assert_eq!(next(None, true, long, 3, false, 0), Fixes::Wait);
        assert_eq!(next(None, true, QUIET, 3, false, 0), Fixes::GoOn);
    }

    #[test]
    fn a_plan_needs_its_fixes_section_filled() {
        assert!(missing("## Fixes\n- Stop at n in src/a.rs\n", &[]).is_empty());
        // One subheading a fix, with no line of its own above them.
        assert!(missing("## Fixes\n\n### r1-1: Drops it\n\nCause.\n", &[]).is_empty());
        assert_eq!(missing("# Plan\n\nNothing.\n", &[]).len(), 1);
        assert_eq!(missing("## Fixes\n\n## Questions\n- Why?\n", &[]).len(), 1);
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
