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
//! AI review's findings taken as they are, with no word of the user's, skip
//! the planning: gerdr writes the plan from them and the fixes go at once
//! (`review_import::needs_plan`). So do Verify's failures, which hold none
//! (`verify::fixes_plan`); Plan fixes still asks for one. The rest of the
//! round is the same.
//!
//! A round that is over keeps its plan to read, not to change. It can be
//! sent again only by its name, with nothing new to send where the feature
//! is: its boxes are unticked then, so it ends once the agent ticks them anew.
//!
//! The plan must speak of everything the round was sent. gerdr lists its
//! fixes in the build log, apart from the milestones, and the round is over
//! once every one is ticked: an agent resting before that is told to go on,
//! and one that won't is the user's to send again or drop. A tick also marks
//! the AI review findings it covers (`mark_made`), so that stage's page shows
//! them made as they go.
//!
//! This is the round's record and what decides its next step. Gerdr's side
//! is `crate::feature_fix`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::{
    Stage, artifacts,
    review_import::{AiReview, FindingState},
    verify::{Evidence, VerifyRun},
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
/// What a round's heading in the build log says before its number.
const ROUND: &str = "Fix round";

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
    /// What the agent must tick in the build log for the round to end: read
    /// from its plan once that waits, and settled when the fixes go.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    /// How many of them were ticked when last read.
    #[serde(default)]
    pub done: u32,
    /// Unix seconds it was over, its fixes made: none while open, and on one
    /// over before this was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
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

    /// How far its fixes got, once over, as its card says it: "2 of 2 made".
    pub fn made(&self) -> String {
        match self.items.len() {
            0 => SENT.to_owned(),
            total => format!("{} of {total} made", (self.done as usize).min(total)),
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

    /// The fixes its plan lists, to show ahead of the build log: none while
    /// the plan is being written, as an older reading no longer holds then.
    pub fn planned(&self) -> &[String] {
        match self.phase {
            FixPhase::Compacting | FixPhase::Planning => &[],
            FixPhase::Ready | FixPhase::CompactingToFix | FixPhase::Fixing => &self.items,
        }
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

/// How many lines saying why a check failed a `VerifyFix` keeps.
const WHY_LINES: usize = 2;

/// A fix round Verify's failures started, kept for Build's page once the run
/// and the round are gone: `VerifyRun` and `FixRound` only hold the latest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyFix {
    /// `FixRound::n`.
    pub round: u32,
    /// The Verify run that failed.
    pub run: u32,
    /// Unix seconds that run ended.
    pub at: u64,
    /// The Build agent it went to; empty if it has none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// What failed (`VerifyRun::failures`), the check `why` explains first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
    /// The first lines saying why the first failure failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub why: Vec<String>,
    /// The user dropped the round.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dropped: bool,
    /// The last run since that said something of the code, and whether it
    /// passed. It stays as it was once a newer round came.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<(u32, bool)>,
}

impl VerifyFix {
    /// What it is called on Build's page.
    pub fn label(&self) -> String {
        format!("Verify {}", self.round)
    }

    /// What round `round` keeps of failed run `run`, going to `agent`.
    pub fn new(round: u32, run: &VerifyRun, agent: String) -> VerifyFix {
        let mut failures = run.failures.clone();
        // Not every failed check prints a line that says why.
        let explained = (run.checks.iter())
            .filter_map(|check| check.outcome.as_ref())
            .filter(|outcome| !outcome.passed() && !outcome.excerpt.is_empty())
            .find_map(|outcome| {
                let own = format!("{}: `{}`", outcome.repo, outcome.command);
                let at = failures.iter().position(|f| f.starts_with(&own))?;
                Some((at, &outcome.excerpt))
            });
        let why = explained.map_or_else(Vec::new, |(at, excerpt)| {
            failures[..=at].rotate_right(1);
            excerpt.iter().take(WHY_LINES).cloned().collect()
        });
        VerifyFix {
            round,
            run: run.n,
            at: run.finished.unwrap_or(run.started),
            agent,
            failures,
            why,
            dropped: false,
            next: None,
        }
    }
}

/// Note on the newest of `fixes` how Verify's run `n` went, if that says
/// something of the code: not when nothing ran, or a check was cut short.
pub fn verify_ended(fixes: &mut [VerifyFix], n: u32, evidence: &Evidence) {
    if evidence.outcomes.is_empty() || evidence.interrupted() {
        return;
    }
    // Only the newest: what fixed an older one is told by the run after it.
    // Its fixes can go again, so a later run replaces an earlier one's word.
    if let Some(fix) = fixes.last_mut().filter(|fix| fix.run < n) {
        fix.next = Some((n, evidence.passed()));
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
    format!("{ROUND} {n}")
}

/// What round `n`'s list is called on Build's page, under the plan's own: by
/// who asked, as `fixes` and `asked` say. AI review's read "Review N", and so
/// does a round from before they were kept.
pub fn label(n: u32, fixes: &[VerifyFix], asked: &BTreeMap<u32, Stage>) -> String {
    if let Some(fix) = fixes.iter().find(|fix| fix.round == n) {
        return fix.label();
    }
    match asked.get(&n) {
        Some(stage @ (Stage::Verify | Stage::CodeReview | Stage::ManualTest)) => {
            format!("{} {n}", stage.label())
        }
        _ => format!("Review {n}"),
    }
}

/// The number of round whose section `line` of a build log heads.
fn round_of(line: &str) -> Option<u32> {
    let rest = line.trim().strip_prefix("## ")?.strip_prefix(ROUND)?;
    rest.trim().parse().ok()
}

/// What each round listed in build log `log`, oldest first: its number, and
/// each box's text and whether it is ticked. A section with no box is left
/// out.
pub fn rounds(log: &str) -> Vec<(u32, Vec<(bool, String)>)> {
    let mut found: Vec<u32> = log.lines().filter_map(round_of).collect();
    found.sort_unstable();
    found.dedup();
    (found.into_iter())
        .map(|n| (n, artifacts::items_under(log, &heading(n))))
        .filter(|(_, items)| !items.is_empty())
        .collect()
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

/// Build log `log` with round `n`'s boxes unticked, as its fixes are sent
/// again: it is over only once the agent ticks each anew. What it wrote
/// after a box stays.
pub fn untick(log: &str, n: u32) -> String {
    let own = format!("## {}", heading(n));
    let mut inside = false;
    let out: Vec<String> = (log.lines())
        .map(|line| {
            if line.starts_with("# ") || line.starts_with("## ") {
                inside = line.trim() == own;
            }
            match artifacts::checkbox(line.trim_start()) {
                Some(mark) if inside && mark != "[ ]" => line.replacen(mark, "[ ]", 1),
                _ => line.to_owned(),
            }
        })
        .collect();
    format!("{}\n", out.join("\n"))
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

/// Build log `log` without the unticked boxes of round `n`, dropped: what
/// was made of it stays, as its commits do, and with nothing made its section
/// goes too.
pub fn without_unticked(log: &str, n: u32) -> String {
    let own = format!("## {}", heading(n));
    let lines: Vec<&str> = log.lines().collect();
    let Some(start) = lines.iter().position(|line| line.trim() == own) else {
        return log.to_owned();
    };
    let next = (lines[start + 1..].iter())
        .position(|line| line.starts_with("# ") || line.starts_with("## "))
        .map_or(lines.len(), |i| start + 1 + i);
    let body: Vec<&str> = (lines[start + 1..next].iter().copied())
        .filter(|line| artifacts::checkbox(line.trim_start()) != Some("[ ]"))
        .collect();
    let mut out: Vec<&str> = lines[..start].to_vec();
    if body.iter().all(|line| line.trim().is_empty()) {
        // The blank line before it goes with it.
        if next == lines.len() {
            while out.last().is_some_and(|line| line.trim().is_empty()) {
                out.pop();
            }
        }
    } else {
        out.push(lines[start]);
        out.extend(body);
    }
    out.extend_from_slice(&lines[next..]);
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

/// Mark the findings `round` was sent as `left` says: a finding's fix is made
/// once every item naming it is ticked, and `items` gives each one at least
/// one. Says whether any changed.
pub fn mark_made(review: &mut AiReview, round: &FixRound, left: &[String]) -> bool {
    let GiveBack::Findings { ids, .. } = &round.give_back else {
        return false;
    };
    let mut changed = false;
    for finding in &mut review.findings {
        if finding.state != FindingState::Sent || !ids.contains(&finding.id) {
            continue;
        }
        let id = &finding.id;
        let mut named = round.items.iter().filter(|item| names(item, id)).peekable();
        let made = named.peek().is_some() && !named.any(|item| left.contains(item));
        changed |= finding.made != made;
        finding.made = made;
    }
    changed
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
    fn a_round_sent_again_is_unticked_and_not_over() {
        let log = "## Milestones\n\n- [x] Build it\n\n## Fix round 2\n\n- [x] `a.rs:3` committed 1a2b\n- [X] Second\n- [ ] Third\n\n## Fix round 3\n\n- [x] Other\n";
        let items = ["`a.rs:3`", "Second", "Third"].map(str::to_owned);
        assert_eq!(left(log, 2, &items), ["Third"]);
        let again = untick(log, 2);
        assert_eq!(left(&again, 2, &items), items);
        // What the agent wrote stays, and other sections are as they were.
        assert!(again.contains("- [ ] `a.rs:3` committed 1a2b"));
        assert!(again.contains("- [x] Build it") && again.contains("- [x] Other"));
        // Its boxes stand as they are, so nothing is added.
        assert_eq!(with_items(&again, 2, &items), again);
    }

    #[test]
    fn a_rounds_label_says_who_asked() {
        let asked = BTreeMap::from([
            (1, Stage::AiReview),
            (2, Stage::CodeReview),
            (3, Stage::ManualTest),
            (4, Stage::Verify),
        ]);
        let label = |n, fixes: &[VerifyFix]| label(n, fixes, &asked);
        assert_eq!(label(1, &[]), "Review 1");
        assert_eq!(label(2, &[]), "Code review 2");
        assert_eq!(label(3, &[]), "Manual test 3");
        assert_eq!(label(4, &[]), "Verify 4");
        // From before they were kept.
        assert_eq!(label(9, &[]), "Review 9");
        let kept = VerifyFix {
            round: 9,
            ..VerifyFix::default()
        };
        assert_eq!(label(9, &[kept]), "Verify 9");
    }

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
            ended: None,
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
    fn each_rounds_list_is_read_oldest_first() {
        let ticked = |text: &str| (true, text.to_owned());
        let open = |text: &str| (false, text.to_owned());
        let log = "## Milestones\n\n- [x] One\n\n## Fix round 10\n\n- [ ] r10-1\n\n## Fix round 2\n\n- [x] r2-1: a, commit abc\n\n  Note.\n\n- [ ] r2-2\n\n## Fix round 3\n\nNo box.\n\n## Fix round x\n\n- [ ] no\n";
        assert_eq!(
            rounds(log),
            [
                (2, vec![ticked("r2-1: a, commit abc"), open("r2-2")]),
                (10, vec![open("r10-1")]),
            ]
        );
        assert!(rounds("## Milestones\n- [ ] One\n").is_empty());
        let fixes = [VerifyFix {
            round: 3,
            ..VerifyFix::default()
        }];
        assert_eq!(label(2, &fixes, &BTreeMap::new()), "Review 2");
        assert_eq!(label(3, &fixes, &BTreeMap::new()), "Verify 3");
    }

    #[test]
    fn a_dropped_round_keeps_only_what_was_made() {
        let log = "## Milestones\n\n- [ ] One\n\n## Fix round 1\n\n- [x] r1-1, abc\n\n  Note.\n\n- [ ] r1-2\n\n## Fix round 2\n\n- [ ] r2-1\n";
        let tidy = without_unticked(log, 1);
        assert_eq!(
            tidy,
            "## Milestones\n\n- [ ] One\n\n## Fix round 1\n\n- [x] r1-1, abc\n\n  Note.\n\n\n## Fix round 2\n\n- [ ] r2-1\n"
        );
        // Nothing made: the section goes, and the rest is as it was.
        assert_eq!(
            without_unticked(&tidy, 2),
            "## Milestones\n\n- [ ] One\n\n## Fix round 1\n\n- [x] r1-1, abc\n\n  Note.\n"
        );
        let between = without_unticked("## Fix round 1\n\n- [ ] a\n\n## Notes\n\nHi\n", 1);
        assert_eq!(between, "## Notes\n\nHi\n");
        // No such round: as it was.
        assert_eq!(without_unticked(log, 9), log);
    }

    #[test]
    fn a_finding_is_made_once_every_item_naming_it_is_ticked() {
        use super::super::review_import::Finding;
        let finding = |id: &str, state| Finding {
            id: id.to_owned(),
            state,
            ..Finding::default()
        };
        let mut review = AiReview {
            findings: vec![
                finding("r1-1", FindingState::Sent),
                finding("r1-2", FindingState::Sent),
                finding("r1-3", FindingState::Sent),
                // r1-11 isn't r1-1, and this one went in no round of these.
                finding("r1-11", FindingState::Sent),
                finding("r1-4", FindingState::Open),
            ],
            ..AiReview::default()
        };
        let ids: Vec<String> = ["r1-1", "r1-2", "r1-3", "r1-4"].map(str::to_owned).into();
        let items: Vec<String> = ["r1-1 and r1-2: one cause", "r1-2: its test", "r1-3: a typo"]
            .map(str::to_owned)
            .into();
        let round = FixRound {
            n: 1,
            what: String::new(),
            input: PathBuf::new(),
            stage: Stage::AiReview,
            phase: FixPhase::Fixing,
            sent_back: 0,
            asked: 0,
            ready: None,
            stalled: None,
            give_back: GiveBack::Findings {
                ids,
                comments: None,
            },
            items: items.clone(),
            done: 0,
            ended: None,
        };
        let made = |review: &AiReview| -> Vec<String> {
            (review.findings.iter())
                .filter(|f| f.fix_made())
                .map(|f| f.id.clone())
                .collect()
        };
        assert!(!mark_made(&mut review, &round, &items));
        assert!(made(&review).is_empty());
        // One item of two for r1-2, the only one for r1-1.
        assert!(mark_made(&mut review, &round, &items[1..]));
        assert_eq!(made(&review), ["r1-1"]);
        // As it was: nothing to save.
        assert!(!mark_made(&mut review, &round, &items[1..]));
        assert!(mark_made(&mut review, &round, &[]));
        assert_eq!(made(&review), ["r1-1", "r1-2", "r1-3"]);
        // A box unticked again takes it back.
        assert!(mark_made(&mut review, &round, &items[2..]));
        assert_eq!(made(&review), ["r1-1", "r1-2"]);
        // Past Build, the word goes whatever the flag says.
        review.findings[0].state = FindingState::Fixed;
        assert_eq!(made(&review), ["r1-2"]);
        // A round of something else marks nothing.
        let other = FixRound {
            give_back: GiveBack::Nothing,
            ..round
        };
        assert!(!mark_made(&mut review, &other, &items));
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
    fn a_verify_fix_puts_the_failure_it_explains_first() {
        let failed =
            |repo: &str, command: &str, excerpt: &[&str]| crate::feature::verify::CheckRun {
                outcome: Some(crate::feature::verify::Outcome {
                    repo: repo.into(),
                    command: command.into(),
                    exit: Some(1),
                    excerpt: excerpt.iter().map(|l| (*l).to_owned()).collect(),
                    ..Default::default()
                }),
                ..Default::default()
            };
        let run = VerifyRun {
            n: 4,
            started: 100,
            finished: Some(160),
            failures: vec![
                "uncommitted changes in web".to_owned(),
                "api: `lint` exited with 1".to_owned(),
                "web: `test` exited with 1".to_owned(),
            ],
            checks: vec![
                failed("api", "lint", &[]),
                failed("web", "test", &["one", "two", "three"]),
            ],
            ..Default::default()
        };
        let fix = VerifyFix::new(3, &run, "Claude Code".to_owned());
        assert_eq!((fix.round, fix.run, fix.at), (3, 4, 160));
        assert_eq!(
            fix.failures,
            [
                "web: `test` exited with 1",
                "uncommitted changes in web",
                "api: `lint` exited with 1"
            ]
        );
        assert_eq!(fix.why, ["one", "two"]);
        // Nothing says why: they stay as they were.
        let run = VerifyRun {
            checks: vec![failed("api", "lint", &[])],
            ..run
        };
        let fix = VerifyFix::new(3, &run, String::new());
        assert_eq!(fix.failures[0], "uncommitted changes in web");
        assert!(fix.why.is_empty());
    }

    #[test]
    fn only_a_run_that_says_something_of_the_code_counts_as_the_next() {
        let outcome = |exit| crate::feature::verify::Outcome {
            exit,
            ..Default::default()
        };
        let evidence = |outcomes| Evidence {
            outcomes,
            ..Evidence::default()
        };
        let fix = |round, run| VerifyFix {
            round,
            run,
            ..VerifyFix::default()
        };
        let mut fixes = vec![fix(1, 2)];
        // Nothing ran: uncommitted changes, say.
        verify_ended(&mut fixes, 3, &evidence(Vec::new()));
        assert_eq!(fixes[0].next, None);
        let cut = crate::feature::verify::Outcome {
            interrupted: true,
            ..outcome(None)
        };
        verify_ended(&mut fixes, 3, &evidence(vec![cut]));
        assert_eq!(fixes[0].next, None);
        // The run it came from says nothing of what it fixed.
        verify_ended(&mut fixes, 2, &evidence(vec![outcome(Some(0))]));
        assert_eq!(fixes[0].next, None);
        verify_ended(&mut fixes, 3, &evidence(vec![outcome(Some(1))]));
        assert_eq!(fixes[0].next, Some((3, false)));
        // Its fixes sent again: the later run says.
        verify_ended(&mut fixes, 4, &evidence(vec![outcome(Some(1))]));
        assert_eq!(fixes[0].next, Some((4, false)));
        // Later ones go to the newer round.
        fixes.push(fix(2, 3));
        verify_ended(&mut fixes, 4, &evidence(vec![outcome(Some(0))]));
        assert_eq!(fixes[0].next, Some((4, false)));
        assert_eq!(fixes[1].next, Some((4, true)));
        // An older round left without a next doesn't take a later run's.
        let mut fixes = vec![
            fix(1, 2),
            VerifyFix {
                next: Some((4, true)),
                ..fix(2, 3)
            },
        ];
        verify_ended(&mut fixes, 5, &evidence(vec![outcome(Some(0))]));
        assert_eq!(fixes[0].next, None);
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
