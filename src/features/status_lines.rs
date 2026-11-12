//! What the feature panel says: the state it draws from, card and stage lines, the rail.

use super::*;

/// What the feature panel shows of a feature besides the feature itself.
#[derive(Default)]
pub(super) struct PanelState<'a> {
    /// What the page says of a feature another machine writes: the words, and
    /// whether something is wrong.
    pub(super) seen_note: Option<(String, bool)>,
    /// The repositories being touched, by `Repo::key`.
    pub(super) busy: &'a [&'a str],
    /// The feature's projects' names, for its repositories' labels.
    pub(super) names: feature::PerProject<String>,
    /// Its latest probe.
    pub(super) probe: Option<&'a Probe>,
    /// Its pull requests are being opened.
    pub(super) shipping: bool,
    /// What its current stage's agent does, for that stage's row (see
    /// `feature_agents::agent_line`).
    pub(super) agent: Option<String>,
    /// Its current stage's agent's status, while its tab is there.
    pub(super) agent_status: Option<AgentStatus>,
    /// Whose turn it is with that agent (see `Gerdr::agent_standing_of`).
    pub(super) agent_standing: Option<chrome::Standing>,
    /// gerdr reads what that agent wrote (`feature_agents::Watch::reads`):
    /// it waits on gerdr, not on the user.
    pub(super) agent_read: bool,
    /// The last lines of its current stage's agent's pane, as last read.
    pub(super) tail: &'a [String],
    /// What its rows need besides it.
    pub(super) ctx: RowCtx,
    /// What its setup does now, while it runs.
    pub(super) setup: Option<&'a str>,
    /// What is being done to its sandbox now.
    pub(super) sandbox: Option<&'a str>,
    /// What Verify's checks do now, while they run.
    pub(super) verify: Option<&'a str>,
    /// The checks its project gives Verify, and what the feature skips.
    pub(super) plan: Vec<PlannedCheck>,
    /// The last lines of the panes of Verify's running checks, as last read.
    pub(super) check_tails: Option<&'a HashMap<String, AgentTail>>,
    /// What Code review's row says once the diff was read (see
    /// `code_review_line`).
    pub(super) code_review: Option<String>,
    /// That line's figures, once its diff was read.
    pub(super) code_counts: Option<crate::feature_diff::Counts>,
    /// How far Manual test's criteria went, in words, when the brief has
    /// some (see `manual_test::counts_text`).
    pub(super) manual: Option<String>,
    /// How many of Manual test's criteria passed, of how many (see
    /// `feature_agents::gate_of`).
    pub(super) manual_gate: Option<(u32, u32)>,
    /// Manual test's criteria, as their cards show them.
    pub(super) criteria: Vec<Row>,
    /// The earlier AI reviews showing their findings (see
    /// `FeaturePanel::rounds_open`).
    pub(super) rounds_open: BTreeSet<u32>,
    /// The fix round cards flipped from how they show by default (see
    /// `FeaturePanel::fix_rounds_flipped`).
    pub(super) fix_rounds_flipped: BTreeSet<u32>,
    /// The chips to pick the agent of the stage on the page with (see
    /// `feature_agents::stage_chips`).
    pub(super) picks: Vec<AgentChip>,
    /// The button picking its model, where gerdr can start the agent on
    /// one (see `feature_agents::stage_model_slot`).
    pub(super) model: Option<String>,
    /// The same for the agent fixes go to (Build's), on a page sending
    /// them (`keys::fix_page`).
    pub(super) fix_picks: Vec<AgentChip>,
    pub(super) fix_model: Option<String>,
    /// Whether the next fixes start a fresh agent, not the one open
    /// (`Gerdr::stage_agent_moved`).
    pub(super) fix_moved: bool,
    /// What the button placing the next agent says, with machines to place it
    /// on (see `machine_work::place_label`).
    pub(super) place: Option<String>,
    /// Where the current agent runs, if on another machine, and whether that
    /// says all there is to say of it (see `machine_work::machine_line`).
    pub(super) machine_line: Option<(String, bool)>,
    /// The machine `Feature::setup` ran on, if not this one.
    pub(super) setup_on: Option<String>,
    /// How its sandbox stands here, when another machine has it.
    pub(super) sandbox_seen: Option<crate::machine_sandbox::SandboxSeen>,
    /// How its sandbox stands on the network, when open to it.
    pub(super) sandbox_lan: Option<crate::sandbox_lan::LanSeen>,
}

/// What the button picking a stage's model says.
pub(super) fn model_label(picked: Option<&str>) -> String {
    format!("Model: {}", picked.unwrap_or("default"))
}

/// `what` a task does as an inline status: "Setting up “T”" reads
/// "setting up “T”"; "services-api: npm ci (2 of 3)" stays.
pub(super) fn inline(what: &str) -> String {
    let mut chars = what.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => first
            .to_lowercase()
            .chain(what[first.len_utf8()..].chars())
            .collect(),
        _ => what.to_owned(),
    }
}

/// The detail of Build's row, not marked done, beside `agent`, its
/// agent's line when Build is current: setup first, while it runs (`setup`
/// says what it does), or why it failed before an agent started. With an
/// agent, its line comes first: what it waits for matters more.
pub(super) fn build_line(
    feature: &Feature,
    setup: Option<&str>,
    agent: Option<&str>,
) -> Option<String> {
    if let Some(what) = setup {
        // The task's text names the feature: its own panel needn't.
        let own = format!("Setting up \u{201c}{}\u{201d}", feature.title);
        let rest = what
            .strip_prefix(&own)
            .map(|r| r.trim_start_matches(':').trim());
        return Some(match rest {
            Some("") => "setting up".to_owned(),
            Some(rest) => format!("setting up: {rest}"),
            None => format!("setting up: {}", inline(what)),
        });
    }
    let failed = feature.setup.as_ref().and_then(|s| s.failed.as_deref());
    match (agent, failed) {
        (Some(agent), Some(_)) => Some(format!("{agent} \u{b7} setup failed")),
        (None, Some(why)) => Some(format!("setup failed: {why}")),
        (agent, None) => agent.map(str::to_owned),
    }
}

/// The detail of Verify's row, not marked done: what its checks do while
/// they run (`running`), that there are none to run, or what failed and
/// who fixes it.
pub(super) fn verify_line(feature: &Feature, ctx: RowCtx, running: Option<&str>) -> Option<String> {
    if let Some(what) = running {
        return Some(inline(what));
    }
    if feature.status == Status::Open
        && feature.current() == Some(Stage::Verify)
        && let Some(said) = ci_line(feature, ctx)
    {
        return Some(said);
    }
    let current = feature.status == Status::Open && feature.current() == Some(Stage::Verify);
    if current && !ctx.checks {
        return Some(match ctx.all_skipped {
            true => "every check is skipped: stop skipping one, or Mark done by hand".into(),
            false => "no checks for its repositories: set some, or Mark done by hand".into(),
        });
    }
    let run = feature
        .verify
        .as_ref()
        .filter(|v| v.passed == Some(false))?;
    let line = match run.failures.first() {
        Some(first) => format!("failed: {first}"),
        None => "failed".to_owned(),
    };
    Some(line)
}

/// The line under repository `repo` of `feature`: its branch, its pull
/// request, and a push not guarded as `probe` found; for one it doesn't
/// change, its base.
pub(super) fn repo_line(feature: &Feature, repo: &feature::Repo, probe: Option<&Probe>) -> String {
    if !repo.touched {
        return format!("context, at {}", repo.base);
    }
    let mut line = repo.branch.clone().unwrap_or_else(|| feature.branch());
    if let Some(pr) = pr_of(feature, &repo.key()) {
        line.push_str(&format!(" \u{b7} PR {}", pr_tail(&pr.url)));
    }
    if let Some(Guard::Off(why)) = probe.and_then(|p| p.guards.get(&repo.key())) {
        line.push_str(&format!(" \u{b7} push not guarded: {why}"));
    }
    let reading = probe.and_then(|p| p.reading.as_ref().ok());
    if let Some((branch, _)) = reading.and_then(|r| r.sits_on.get(&repo.key())) {
        line.push_str(&format!(
            " \u{b7} built on {branch}, not on its base {}",
            repo.base
        ));
    }
    line
}

/// The "!" problems of features whose repositories are built on another
/// branch than their base, one per feature.
pub(super) const BASE_PROBLEM: &str = "feature-base";

/// What a stage of `feature` not marked done says of where it stands,
/// as `state` has it: what setup or the checks do, the agent's line on
/// the current stage, what waits on AI review, Code review and Manual
/// test.
pub(super) fn stage_line(feature: &Feature, stage: Stage, state: &PanelState) -> Option<String> {
    if stage == Stage::Ship
        && feature.pull().is_some()
        && feature.status == Status::Open
        && feature.current() == Some(Stage::Ship)
    {
        return submit_line(feature, state.ctx);
    }
    let current = stage_mark(feature, stage) == StageMark::Current;
    let agent = state.agent.as_deref().filter(|_| current);
    match stage {
        Stage::Build => build_line(feature, state.setup, agent),
        Stage::Verify => verify_line(feature, state.ctx, state.verify),
        Stage::AiReview if current => review_line(feature, agent),
        Stage::CodeReview if current => {
            Some((state.code_review.clone()).unwrap_or_else(|| "review every changed file".into()))
        }
        Stage::ManualTest if current => state.manual.clone(),
        _ => agent.map(str::to_owned),
    }
}

#[cfg(test)]
/// What `stage` of `feature` says in one line: when it was done, else
/// where it stands, else what it is for.
pub(super) fn stage_detail(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    now: u64,
) -> String {
    match (feature.marks.get(&stage), stage_line(feature, stage, state)) {
        (Some(m), _) => mark_detail(m, now),
        (None, Some(line)) => line,
        (None, None) => stage.about().to_owned(),
    }
}

/// The first stage of `feature` whose mark the code changed since, as the
/// latest probe read it.
pub(super) fn stale_from(feature: &Feature, probe: Option<&Probe>) -> Option<Stage> {
    if feature.status != Status::Open {
        return None;
    }
    match probe.map(|p| &p.reading) {
        Some(Ok(reading)) => feature.stale(&reading.patch),
        _ => None,
    }
}

/// Whether done `stage` of `feature` no longer holds: bound to the code,
/// and the code changed since it or a stage before it was marked.
pub(super) fn is_stale(feature: &Feature, stage: Stage, probe: Option<&Probe>) -> bool {
    feature.is_done(stage)
        && stage.bound()
        && stale_from(feature, probe).is_some_and(|from| stage_index(stage) >= stage_index(from))
}

/// How many of Manual test's criteria passed, of how many (see
/// `feature_agents::gate_of`).
pub(super) fn manual_gate(n: crate::feature::manual::Counts) -> (u32, u32) {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    // The user's comments aren't criteria: "3 of 4 passed" counts the
    // brief's, as the bar over the page does, unless there are only comments.
    let criteria = n.total() - n.comments();
    match criteria {
        0 => (count(n.passed), count(n.total())),
        _ => (count(n.passed - n.comments_done), count(criteria)),
    }
}

/// What a current stage's short line reads besides its feature (see
/// `current_line`), on the rail and on the sidebar's cards.
pub(super) struct LineState<'a> {
    pub(super) probe: Option<&'a Probe>,
    pub(super) shipping: bool,
    pub(super) checks: bool,
    pub(super) setup: Option<&'a str>,
    pub(super) verify: Option<&'a str>,
    pub(super) manual_gate: Option<(u32, u32)>,
    /// How far its code review came, once its diff was read.
    pub(super) code_counts: Option<crate::feature_diff::Counts>,
    /// Where its current agent runs, if on another machine, and whether that
    /// says all there is to say of it.
    pub(super) machine: Option<(String, bool)>,
    /// Whose turn it is with its current stage's agent, while its tab is
    /// there (see `Gerdr::agent_standing_of`).
    pub(super) agent: Option<chrome::Standing>,
}

impl<'a> From<&PanelState<'a>> for LineState<'a> {
    fn from(state: &PanelState<'a>) -> Self {
        Self {
            probe: state.probe,
            shipping: state.shipping,
            checks: state.ctx.checks,
            setup: state.setup,
            verify: state.verify,
            manual_gate: state.manual_gate,
            code_counts: state.code_counts,
            machine: state.machine_line.clone(),
            agent: state.agent_standing,
        }
    }
}

/// The stage a card shows, its line and its tone. Work sent back to Build
/// speaks for the card, as it does on the rail (see `Rework::node`).
pub(super) fn card_status(
    feature: &Feature,
    state: &LineState,
    hold: CardHold,
    back: Option<Rework>,
    now: u64,
) -> (Option<Stage>, String, chrome::CardTone) {
    // A round's agent at work says what it does; one waiting for its prompt
    // has the user to act.
    let round = feature.fix_round.as_ref();
    if let Some(back) = back.filter(|b| round.is_none() || !matches!(b, Rework::Runs(_))) {
        let line = format!("{} \u{b7} {}", Stage::Build.label(), back.node().1);
        return (Some(Stage::Build), line, back.tone());
    }
    if let Some(round) = round {
        let tone = match (round.phase, round.stalled.is_some()) {
            (FixPhase::Ready, _) => chrome::CardTone::Waits,
            (_, true) => chrome::CardTone::Failed,
            _ => chrome::CardTone::Working,
        };
        // Where the rail has it: its commits may have sent the flow back.
        let stage = (feature.current())
            .filter(|s| fix_round_on(feature, *s).is_some())
            .unwrap_or(round.stage);
        let line = format!("{} \u{b7} {}", stage.label(), round.line());
        return (Some(stage), line, tone);
    }
    let stage = feature.current();
    let line = card_line(feature, stage, state, now);
    (stage, line, card_tone(feature, stage, state, hold))
}

/// A card's line for `feature` at `stage`: the stage and how it stands,
/// else how long nothing happened to it.
pub(super) fn card_line(
    feature: &Feature,
    stage: Option<Stage>,
    state: &LineState,
    now: u64,
) -> String {
    let Some(stage) = stage else {
        return "Every stage done".to_owned();
    };
    let mut what = current_line(feature, stage, state, now);
    if what.is_empty() {
        let last = feature.history.iter().map(|e| e.at).max();
        what = crate::clock::ago(now, last.unwrap_or(feature.created));
    }
    match &state.machine {
        Some((line, true)) => what.clone_from(line),
        Some((line, false)) => what = format!("{what} \u{b7} {line}"),
        None => {}
    }
    format!("{} \u{b7} {what}", stage.label())
}

/// What holds `feature` or keeps it busy, for its card's tone.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CardHold {
    /// A gate holds it.
    pub(super) gate: bool,
    /// A job of it runs.
    pub(super) busy: bool,
}

/// How `feature` at `stage` stands, for its card, as `hold` says.
pub(super) fn card_tone(
    feature: &Feature,
    stage: Option<Stage>,
    state: &LineState,
    hold: CardHold,
) -> chrome::CardTone {
    let CardHold { gate, busy } = hold;
    let run = stage.and_then(|stage| feature.runs.get(&stage));
    let failed =
        |stage| stage_failed(feature, stage, state.probe) || agent_failed(feature, stage, state);
    if stage.is_some_and(failed) {
        chrome::CardTone::Failed
    } else if gate || stage.is_some_and(|stage| agent_yours(feature, stage, state)) {
        chrome::CardTone::Waits
    } else if busy || state.agent == Some(chrome::Standing::Working) {
        chrome::CardTone::Working
    } else if state.agent.is_none() && run.is_some_and(feature::agents::StageRun::live) {
        // Its agent's state isn't known yet: it was started, so it works.
        chrome::CardTone::Working
    } else {
        chrome::CardTone::Idle
    }
}

/// Whether `stage` of `feature`, the one it is at, failed or is blocked: its
/// setup failed before an agent started, its Build agent said it is
/// blocked, Verify's checks failed, a fix plan it asked for stalled, Ship
/// is held up. The rail, the sidebar's card and the page all say so in the
/// error colour (`Standing::Failed`).
pub(super) fn stage_failed(feature: &Feature, stage: Stage, probe: Option<&Probe>) -> bool {
    if feature.status != Status::Open {
        return false;
    }
    let run = feature.runs.get(&stage);
    let stalled =
        (feature.fix_round.as_ref()).is_some_and(|r| r.stage == stage && r.stalled.is_some());
    stalled
        || match stage {
            Stage::Build => {
                let setup = (feature.setup.as_ref()).is_some_and(|s| s.failed.is_some());
                (run.is_none() && setup) || run.is_some_and(|r| r.blocked.is_some())
            }
            Stage::Verify => (feature.verify.as_ref()).is_some_and(|v| v.passed == Some(false)),
            Stage::Ship => ship_planned(feature, probe).is_some_and(|p| !p.blockers.is_empty()),
            _ => false,
        }
}

/// Whether the agent of `stage`, where `feature` is at, waits on the user
/// (`feature_agents::agent_standing`).
fn agent_yours(feature: &Feature, stage: Stage, state: &LineState) -> bool {
    feature.current() == Some(stage) && state.agent == Some(chrome::Standing::Yours)
}

/// Whether the agent of `stage`, where `feature` is at, left its tab with its
/// work not done (`feature_agents::agent_standing`).
fn agent_failed(feature: &Feature, stage: Stage, state: &LineState) -> bool {
    feature.current() == Some(stage) && state.agent == Some(chrome::Standing::Failed)
}

/// Whether the current `stage` of `feature` waits for the user: a file to
/// review, a stop to go on from, findings to decide (see
/// `feature_agents::gate_of`).
pub(super) fn waits(feature: &Feature, stage: Stage, state: &LineState) -> bool {
    crate::feature_agents::gate_of(feature, |_| state.manual_gate).is_some_and(|g| g.0 == stage)
}

/// "working 14m": how long an agent started at `started` has worked.
fn running_for(now: u64, started: u64) -> String {
    let word = chrome::Standing::Working.word();
    match crate::clock::ago(now, started).strip_suffix(" ago") {
        Some(long) => format!("{word} {long}"),
        None => word.to_owned(),
    }
}

/// The short line under current `stage`'s node on the rail.
pub(super) fn current_line(feature: &Feature, stage: Stage, state: &LineState, now: u64) -> String {
    if let Some(round) = fix_round_on(feature, stage) {
        let waits = (feature.runs.get(&Stage::Build)).is_some_and(|run| run.waits_to_send());
        return match waits {
            true => "waiting to send".to_owned(),
            false => round.line(),
        };
    }
    let run = feature.runs.get(&stage);
    let agent = || match run {
        Some(run) if run.t3 == Some(feature::agents::T3Run::Draft) => "draft in T3 Code".to_owned(),
        Some(run) if run.waits_to_send() => "waiting to send".to_owned(),
        // Whose turn it is, as the agent's card says it.
        Some(run) if run.live() => match state.agent {
            Some(
                standing @ (chrome::Standing::Yours
                | chrome::Standing::Done
                | chrome::Standing::Failed),
            ) => standing.word().to_owned(),
            _ => running_for(now, run.started),
        },
        Some(_) => "agent closed".to_owned(),
        None => String::new(),
    };
    let failed_setup = (feature.setup.as_ref()).is_some_and(|s| s.failed.is_some());
    match stage {
        Stage::Discover | Stage::Plan if run.is_some_and(|r| r.ready.is_some()) => {
            "ready for review".into()
        }
        Stage::PlanReview => match crate::feature_plan_review::deciding(feature) {
            Some(review) if review.open() > 0 => format!("{} to decide", review.open()),
            Some(_) => "all decided".into(),
            None if run.is_some_and(|r| r.ready.is_some()) => "ready for review".into(),
            None => agent(),
        },
        Stage::Build if state.setup.is_some() => "setting up".into(),
        Stage::Build if failed_setup && run.is_none() => "setup failed".into(),
        Stage::Build if run.is_some_and(|r| r.blocked.is_some()) => "blocked".into(),
        Stage::Build => match run.and_then(|r| r.stop) {
            Some(stop) => format!("milestone {}/{}", stop.done, stop.total),
            None => agent(),
        },
        Stage::Verify if state.verify.is_some() => "running".into(),
        Stage::Verify => match &feature.verify {
            Some(v) if v.passed == Some(false) => "failed".into(),
            _ if !state.checks => "mark by hand".into(),
            _ => String::new(),
        },
        Stage::AiReview => {
            let read = (feature.review.as_ref()).filter(|_| run.is_some_and(|r| r.ready.is_some()));
            match read {
                Some(r) if r.count(FindingState::Open) > 0 => {
                    format!("{} to decide", r.count(FindingState::Open))
                }
                Some(r) if r.count(FindingState::Fix) > 0 => {
                    let wanted = words_in(ItemOf::Finding, r.posts).wanted;
                    format!("{} {wanted}", r.count(FindingState::Fix))
                }
                Some(_) => "findings read".into(),
                None => agent(),
            }
        }
        Stage::CodeReview => match state.code_counts.filter(|c| c.files > 0) {
            Some(c) => format!("{} of {} files", c.viewed, c.files),
            None => "to review".into(),
        },
        Stage::ManualTest => match state.manual_gate {
            Some((passed, total)) if total > 0 => format!("{passed} of {total} passed"),
            _ => String::new(),
        },
        Stage::Ship if feature.pull().is_some() => match feature.shipping.reviews.last() {
            Some(r) => r.event.done().to_owned(),
            None => "to submit".into(),
        },
        Stage::Ship => {
            let planned = ship_planned(feature, state.probe);
            if state.shipping {
                "shipping".into()
            } else if feature.shipping.squash_at.is_some() {
                "merging".into()
            } else if feature.shipping.resolving.is_some() {
                "resolving conflicts".into()
            } else if planned.as_ref().is_some_and(|p| !p.blockers.is_empty()) {
                "blocked".into()
            } else if planned.as_ref().is_some_and(|p| p.nothing) {
                "nothing ticked".into()
            } else if (state.probe)
                .is_some_and(|p| p.bases.values().any(|b| !b.conflicts.is_empty()))
            {
                "base conflicts".into()
            } else if planned.is_some() {
                "ready to ship".into()
            } else {
                String::new()
            }
        }
        Stage::Discover | Stage::Plan => agent(),
    }
}

/// The stage last worked on, when the flow has since gone back before it:
/// of what ran after the current stage, what ran last. The stage the flow
/// left is kept as of then, older marks as of when they were made.
pub(super) fn left_off(feature: &Feature) -> Option<Stage> {
    let current = feature.current()?;
    (feature.ran_once.iter())
        .filter(|(stage, _)| **stage > current)
        .max_by_key(|(stage, at)| (**at, **stage))
        .map(|(stage, _)| *stage)
}

/// How `stage` of `feature` reads, on the rail and in the dots of its
/// sidebar card alike.
pub(super) fn node_mark(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
    state: &LineState,
) -> NodeMark {
    let at_work = rework(feature, ctx);
    if let Some(back) = at_work.filter(|_| stage == Stage::Build) {
        return back.node().0;
    }
    let turned = turned_back(feature, ctx);
    match stage_mark(feature, stage) {
        // Code changing is what's expected now: "code changed" says so once
        // the work is over, when it asks for something. What comes before
        // Build isn't bound to the code, so it stands as it was.
        StageMark::Done if turned.is_some() && stage.bound() => NodeMark::Ran,
        StageMark::Done if is_stale(feature, stage, state.probe) => NodeMark::Stale,
        StageMark::Done => NodeMark::Done,
        StageMark::Current if stage_failed(feature, stage, state.probe) => NodeMark::Failed,
        StageMark::Current if agent_failed(feature, stage, state) => NodeMark::Failed,
        StageMark::Current if waits(feature, stage, state) => NodeMark::Waiting,
        // Its agent rests with its work not done: the user's turn.
        StageMark::Current if agent_yours(feature, stage, state) => NodeMark::Waiting,
        StageMark::Current if turned == Some(stage) && at_work.is_some() => NodeMark::Paused,
        StageMark::Current => NodeMark::Current,
        StageMark::Later if left_off(feature) == Some(stage) => NodeMark::Paused,
        StageMark::Later if feature.ran_once.contains_key(&stage) => NodeMark::Ran,
        StageMark::Later => NodeMark::Later,
    }
}

/// `stage`'s node on the rail of `feature`.
pub(super) fn rail_node(feature: &Feature, stage: Stage, state: &PanelState, now: u64) -> RailNode {
    let mark = stage_mark(feature, stage);
    let ran = mark == StageMark::Later && feature.ran_once.contains_key(&stage);
    let done = mark == StageMark::Done || ran;
    let agent = (feature.runs.get(&stage).filter(|_| done)).map(|run| {
        let what = run.model.as_deref().map_or(run.agent.as_str(), short_model);
        (AgentIcon::from_agent(&run.agent), what.to_owned())
    });
    let lines = LineState::from(state);
    let mark = node_mark(feature, stage, state.ctx, &lines);
    let back = rework(feature, state.ctx).filter(|_| stage == Stage::Build);
    let line = match (stage_mark(feature, stage), back) {
        (_, Some(back)) => back.node().1,
        (StageMark::Done, _) if mark == NodeMark::Stale => "code changed".to_owned(),
        (StageMark::Done, _) => {
            let at = feature.marks.get(&stage).map_or(0, |m| m.at);
            crate::clock::ago(now, at)
        }
        (StageMark::Current, _) => current_line(feature, stage, &lines, now),
        (StageMark::Later, _) => {
            (feature.ran_once.get(&stage)).map_or_else(String::new, |at| crate::clock::ago(now, *at))
        }
    };
    RailNode {
        label: stage.label().to_owned(),
        line,
        mark,
        selected: false,
        agent,
    }
}

/// `model` where its agent's logo stands beside it: Claude's says the first
/// word of its names.
pub(super) fn short_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

/// A line in `tone`.
/// The label of the button moving a pull request's review to its new head.
pub(super) const UPDATE_HEAD: &str = "Update to new head";

/// How many things a review of `feature` would post now, as `ctx` read
/// them: findings to comment, comments of Code review, Manual test's.
fn to_post(feature: &Feature, ctx: RowCtx) -> usize {
    let findings = (feature.review.iter().flat_map(|r| &r.findings))
        .filter(|f| f.state == FindingState::Fix)
        .count();
    findings + ctx.comments + ctx.manual.failed.saturating_sub(ctx.manual.sent)
}

/// What Enter does on Ship's page of `feature`, a pull request's review:
/// push the user's own fixes, post what waits, finish once a review went,
/// else approve someone else's.
pub(super) fn submit_action(feature: &Feature, ctx: RowCtx) -> PanelOutcome {
    use feature::pr_submit::Event;
    if feature.pull().is_none() {
        return PanelOutcome::Tick;
    }
    let works = feature.pulls().iter().any(|p| p.works_here());
    let waiting = to_post(feature, ctx) > 0;
    match () {
        _ if works && !ctx.on_head => PanelOutcome::PushPr,
        _ if waiting => PanelOutcome::SubmitReview(Event::Comment),
        // Pushed, a review is up to the user: Finish needs none.
        _ if works || !feature.shipping.reviews.is_empty() => PanelOutcome::FinishReview,
        _ => PanelOutcome::SubmitReview(Event::Approve),
    }
}

/// The buttons of Ship's page of `feature`, a pull request's review, at
/// it: each way to submit it GitHub takes of it, its text, pushing the
/// user's own, finishing.
fn submit_actions(feature: &Feature, ctx: RowCtx) -> Vec<(PageButton, PanelOutcome)> {
    use feature::pr_submit::Event;
    if feature.pull().is_none() {
        return Vec::new();
    }
    let works = feature.pulls().iter().any(|p| p.works_here());
    // GitHub takes neither an approval nor a change request of one's own.
    let mine = feature.pulls().iter().all(|p| p.mine);
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    let mut out = Vec::new();
    if works {
        out.push((
            button("Push\u{2026}", ButtonStyle::Plain),
            PanelOutcome::PushPr,
        ));
    }
    let events: &[Event] = match mine {
        true => &[Event::Comment],
        false => &[Event::Comment, Event::Approve, Event::RequestChanges],
    };
    for event in events {
        out.push((
            button(event.label(), ButtonStyle::Plain),
            PanelOutcome::SubmitReview(*event),
        ));
    }
    out.push((
        button("Edit text", ButtonStyle::Quiet),
        PanelOutcome::EditSummary,
    ));
    if works || !feature.shipping.reviews.is_empty() || ctx.on_head && to_post(feature, ctx) == 0 {
        out.push((
            button("Finish\u{2026}", ButtonStyle::Plain),
            PanelOutcome::FinishReview,
        ));
    }
    out
}

/// What Ship's line says of `feature`, a pull request's review.
pub(super) fn submit_line(feature: &Feature, ctx: RowCtx) -> Option<String> {
    feature.pull()?;
    let works = feature.pulls().iter().any(|p| p.works_here());
    let mine = feature.pulls().iter().all(|p| p.mine);
    let n = to_post(feature, ctx);
    let last = feature.shipping.reviews.last();
    Some(match () {
        _ if works && !ctx.on_head => "fixes here not on GitHub: push them".into(),
        _ if n > 0 => format!("{} to post", plural(n, "comment")),
        _ => match last {
            Some(r) => format!(
                "{} on GitHub; finish, or wait for new commits",
                r.event.done()
            ),
            None if mine => "nothing to post: finish".into(),
            None => "nothing to post: approve, or finish".into(),
        },
    })
}

/// Ship's page of `feature`, a pull request's review: what goes, where, and
/// the reviews submitted, holding the buttons of `submit_actions`.
pub(super) fn submit_sections(
    feature: &Feature,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Vec<PageSection> {
    let Some(pull) = feature.pull() else {
        return Vec::new();
    };
    let notes = crate::review::Review {
        comments: notes_of(state.probe).to_vec(),
        ..crate::review::Review::default()
    };
    let items = feature::pr_submit::items(feature, &notes, &[]);
    let mut said: Vec<CheckLine> = (items.iter())
        .map(|item| {
            let at = match &item.spot {
                Some(feature::pr_submit::Spot {
                    path,
                    line: Some((_, n, _)),
                }) => {
                    format!("{path}:{n}")
                }
                Some(spot) => spot.path.clone(),
                None => "in the text".to_owned(),
            };
            line(format!("{at} \u{b7} {}", item.title), CheckTone::Note)
        })
        .collect();
    let manual = state
        .ctx
        .manual
        .failed
        .saturating_sub(state.ctx.manual.sent);
    if manual > 0 {
        let text = format!(
            "in the text \u{b7} {} from Manual test",
            plural(manual, "failure")
        );
        said.push(line(text, CheckTone::Note));
    }
    if said.is_empty() {
        said.push(line("nothing to post yet", CheckTone::Note));
    }
    let mut out = vec![lines("To post", said)];
    let held: Vec<usize> = (actions.iter().enumerate())
        .filter(|(_, (b, o))| {
            b.style != ButtonStyle::Primary
                && matches!(o, PanelOutcome::SubmitReview(_) | PanelOutcome::EditSummary)
        })
        .map(|(i, _)| i)
        .collect();
    if !held.is_empty() {
        out.push(PageSection::Toolbar {
            groups: vec![(String::new(), held)],
        });
    }
    let done: Vec<CheckLine> = (feature.shipping.reviews.iter().rev())
        .map(|r| {
            let short = r.commit.get(..7).unwrap_or(&r.commit);
            let mut text = format!("{} {} at {short}", r.event.done(), crate::clock::ago(now, r.at));
            if r.inline + r.in_body > 0 {
                text.push_str(&format!(
                    " \u{b7} {}",
                    plural(r.inline + r.in_body, "comment")
                ));
            }
            line(text, CheckTone::Ready)
        })
        .collect();
    if !done.is_empty() {
        out.push(lines("Submitted", done));
    }
    if pull.works_here() && !state.ctx.on_head {
        let text = format!("commits here not on {} yet", pull.head_ref);
        out.push(lines("Branch", vec![line(text, CheckTone::Blocker)]));
    }
    out
}

/// `actions` with the button opening `feature`'s pull request in, for a pull
/// request's review (Ctrl+Shift+O). Moving to its new head and finishing
/// once it ended are what Enter does on its current stage's page, so they
/// are there already (`row_action`), and the card holds them.
///
/// At Verify, at the head reviewed, the project's checks run here only
/// when asked: "Run checks here" (Shift+R), as GitHub's are its Verify.
pub(super) fn with_pull_actions(
    actions: &mut Vec<(PageButton, PanelOutcome)>,
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
) {
    if feature.pull().is_none() {
        return;
    }
    if stage == Stage::Ship
        && feature.current() == Some(Stage::Ship)
        && feature.status == Status::Open
        && feature.writer.is_none()
    {
        // No summary of an agent's: the review's text is the user's.
        actions.retain(|(_, o)| *o != PanelOutcome::WriteSummary);
        for (button, outcome) in submit_actions(feature, ctx) {
            match actions.iter_mut().find(|(_, o)| *o == outcome) {
                Some((had, _)) => had.label = button.label,
                None => actions.push((button, outcome)),
            }
        }
    }
    let mut more = Vec::new();
    let theirs = feature.pulls().iter().any(|p| !p.own());
    if theirs
        && feature.pr_ended().is_none()
        && feature.status == Status::Open
        && feature.writer.is_none()
    {
        let fixing = feature.pulls().iter().any(|p| p.fix_here);
        let label = if fixing { COMMENT_INSTEAD } else { FIX_HERE };
        more.push((label, ButtonStyle::Plain, PanelOutcome::FixHere));
    }
    if changes_pulls(feature) {
        more.push((EDIT_PULLS, ButtonStyle::Plain, PanelOutcome::EditPulls));
    }
    for p in feature.pulls() {
        more.push((
            OPEN_PULL,
            ButtonStyle::Quiet,
            PanelOutcome::OpenPr(p.url.clone()),
        ));
    }
    if stage == Stage::Verify
        && ctx.on_head
        && ctx.checks
        && feature.current() == Some(Stage::Verify)
        && feature.status == Status::Open
    {
        more.push((RUN_HERE, ButtonStyle::Plain, PanelOutcome::RunChecks));
    }
    for (label, style, outcome) in more {
        if !actions.iter().any(|(_, o)| *o == outcome) {
            let label = label.to_owned();
            actions.push((PageButton { label, style }, outcome));
        }
    }
}

/// The label of the button running a review's checks here at its head.
pub(super) const RUN_HERE: &str = "Run checks here";

/// Whether `feature`, a pull request's review, is at the head reviewed with
/// nothing changed, as `reading` found its code: every worktree at it,
/// clean.
pub(super) fn on_head(
    feature: &Feature,
    reading: Option<&Result<feature::patch::Reading, String>>,
) -> bool {
    let (true, Some(Ok(reading))) = (feature.pull().is_some(), reading) else {
        return false;
    };
    reading.dirty.is_empty()
        && (feature.repos.iter()).all(|repo| {
            let head = reading.heads.get(&repo.key());
            feature
                .pull_of(&repo.key())
                .is_some_and(|p| head == Some(&p.head_sha))
        })
}

/// What Enter does at Verify of `feature`, a pull request's review at the
/// head reviewed: run the failed checks again on GitHub, open them there
/// while they go (or when they can't run again from here), or mark it done
/// by hand when GitHub has none (it is marked by itself once they pass).
pub(super) fn ci_action(feature: &Feature) -> PanelOutcome {
    use crate::ci::CiState;
    let Some(first) = feature.pull() else {
        return PanelOutcome::Tick;
    };
    // The checks page of the first pull request whose checks hold it up.
    let held = (feature.pulls().iter()).find(|p| {
        p.checks()
            .is_none_or(|c| c.iter().any(|c| c.state != CiState::Passed))
    });
    let open = PanelOutcome::OpenPr(held.unwrap_or(first).checks_url());
    let Some(ci) = feature.pr_ci() else {
        return open;
    };
    let rerun = (feature.pulls().iter())
        .any(|p| !feature::pr::rerun_ids(p.checks().unwrap_or_default()).is_empty());
    match ci.verdict() {
        feature::pr::Verdict::Failed if rerun => PanelOutcome::RerunCi,
        feature::pr::Verdict::Failed | feature::pr::Verdict::Running => open,
        feature::pr::Verdict::None | feature::pr::Verdict::Passed => PanelOutcome::Tick,
    }
}

/// What Verify's line says of `feature`'s checks on GitHub, a pull
/// request's review at the head reviewed.
pub(super) fn ci_line(feature: &Feature, ctx: RowCtx) -> Option<String> {
    feature.pull().filter(|_| ctx.on_head)?;
    let Some(ci) = feature.pr_ci() else {
        return Some("reading its checks on GitHub".into());
    };
    let checks: Vec<&feature::pr::CiCheck> =
        feature.pr_checks().into_iter().map(|(_, c)| c).collect();
    Some(match ci.verdict() {
        feature::pr::Verdict::None => {
            "no checks on GitHub: Mark done by hand, or run checks here".into()
        }
        feature::pr::Verdict::Running => format!(
            "GitHub's checks running: {} of {} done",
            ci.total - ci.running,
            ci.total
        ),
        feature::pr::Verdict::Failed => {
            let first = (checks.iter())
                .find(|c| c.state != crate::ci::CiState::Passed)
                .map_or("", |c| c.name.as_str());
            format!("{} of {} failed on GitHub: {first}", ci.failed, ci.total)
        }
        feature::pr::Verdict::Passed => format!("all {} passed on GitHub", ci.total),
    })
}

/// GitHub's checks of `feature`'s head, a pull request's review, on
/// Verify's page: each with how it stands, and the button opening them
/// there. Once code changed here, they say nothing of it.
pub(super) fn ci_sections(
    feature: &Feature,
    ctx: RowCtx,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    let Some(pull) = feature.pull() else {
        return Vec::new();
    };
    // With several pull requests, each check says whose it is.
    let several = feature.pulls().len() > 1;
    let mut said: Vec<CheckLine> = Vec::new();
    for p in feature.pulls() {
        let of = if several {
            format!("#{} ", p.number)
        } else {
            String::new()
        };
        match p.checks() {
            None => said.push(line(format!("{of}not read yet"), CheckTone::Note)),
            Some([]) => said.push(line(format!("{of}none ran"), CheckTone::Note)),
            Some(checks) => said.extend(checks.iter().map(|c| {
                let (word, tone) = match c.state {
                    crate::ci::CiState::Passed => ("passed", CheckTone::Ready),
                    crate::ci::CiState::Failed => ("failed", CheckTone::Failed),
                    crate::ci::CiState::Running => ("running", CheckTone::Note),
                    crate::ci::CiState::Unknown => ("no result", CheckTone::Blocker),
                };
                line(format!("{of}{}: {word}", c.name), tone)
            })),
        }
    }
    if !ctx.on_head {
        let text = format!(
            "of #{} at {}: the code here isn't that, so its checks run here",
            pull.number,
            pull.short_head()
        );
        said.insert(0, line(text, CheckTone::Note));
    }
    let mut out = vec![lines("GitHub checks", said)];
    let held: Vec<usize> = (actions.iter().enumerate())
        .filter(|(_, (b, o))| match o {
            PanelOutcome::OpenPr(url) => {
                *url == pull.checks_url() && b.style != ButtonStyle::Primary
            }
            PanelOutcome::RunChecks => b.label == RUN_HERE,
            _ => false,
        })
        .map(|(i, _)| i)
        .collect();
    if !held.is_empty() {
        out.push(PageSection::Toolbar {
            groups: vec![(String::new(), held)],
        });
    }
    out
}

/// The label of the button opening a pull request's review's pull request.
pub(super) const OPEN_PULL: &str = "Open pull request";
/// The label of the button adding pull requests to a review, or leaving
/// some out.
pub(super) const EDIT_PULLS: &str = "Pull requests\u{2026}";

/// Whether review `feature` can add or leave out pull requests now: open
/// here, not shipped, and not all ended.
pub(super) fn changes_pulls(feature: &Feature) -> bool {
    !feature.pulls().is_empty()
        && feature.pr_ended().is_none()
        && feature.status == Status::Open
        && feature.writer.is_none()
        && linkable(feature)
}
/// The labels of the button fixing someone else's pull request here, and
/// going back to commenting on it.
pub(super) const FIX_HERE: &str = "Fix here\u{2026}";
pub(super) const COMMENT_INSTEAD: &str = "Comment instead";

/// What hovering button `i` of a pull request review's page says, with its
/// key: one its pull request's card holds.
pub(super) fn pull_tip(
    feature: &Feature,
    stage: Stage,
    page: &chrome::StagePage,
    i: usize,
) -> Option<String> {
    feature.pull()?;
    let held = page.sections.iter().any(|s| {
        matches!(s, PageSection::File { name, buttons, .. }
            if name.starts_with("Pull request #") && buttons.contains(&i))
    });
    let label = page.buttons.get(i)?.label.as_str();
    // The card's primary (moving to the new head, finishing) is the page bar's.
    let primary = page.primary == Some(i) && matches!(label, UPDATE_HEAD | "Finish\u{2026}");
    if !held && !primary {
        return None;
    }
    Some(match label {
        UPDATE_HEAD => "Fetch the new commits and review them, from Verify on (Enter)".to_owned(),
        OPEN_PULL => {
            // Verify's evidence and a fix plan take the key where they are.
            let theirs =
                opens_fix_plan(feature, stage) || (stage == Stage::Verify && has_evidence(feature));
            match theirs {
                true => "Open the pull request on GitHub".to_owned(),
                false => "Open the pull request on GitHub (Ctrl+Shift+O)".to_owned(),
            }
        }
        FIX_HERE => {
            "Fix it here and push the fixes to its branch, instead of commenting (Alt+W)".to_owned()
        }
        COMMENT_INSTEAD => "Comment on it on GitHub instead of fixing it here (Alt+W)".to_owned(),
        EDIT_PULLS => {
            "Add pull requests of other repositories to the review, or leave some out (Alt+N)"
                .to_owned()
        }
        _ => "Finish the review: nothing is posted (Enter)".to_owned(),
    })
}

/// The card of `feature`'s pull request, for a pull request's review, at
/// the top of each of its pages: whose it is, and how it stands on GitHub
/// against its worktree, holding the buttons `with_pull_actions` gave.
pub(super) fn pull_card(
    feature: &Feature,
    pull: &feature::pr::PullRef,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> PageSection {
    // The review's own buttons sit on its first pull request's card.
    let first = feature
        .pull()
        .is_some_and(|p| p.number == pull.number && p.repo == pull.repo);
    let state = if let Some(end) = pull.ended() {
        let how = if end.merged() { "merged" } else { "closed" };
        line(
            format!("{how} on GitHub {}", crate::clock::ago(now, end.at)),
            CheckTone::Note,
        )
    } else if let Some(moved) = pull.moved() {
        let text = match moved.base_ref == pull.base_ref {
            true => format!(
                "new commits on GitHub {}: at {}, reviewed at {}",
                crate::clock::ago(now, moved.at),
                moved.head_sha.get(..7).unwrap_or(&moved.head_sha),
                pull.short_head()
            ),
            false => format!(
                "now into {} on GitHub, reviewed into {}",
                moved.base_ref, pull.base_ref
            ),
        };
        line(text, CheckTone::Blocker)
    } else {
        let by = match pull.author.is_empty() {
            true => String::new(),
            false => format!("by @{} \u{b7} ", pull.author),
        };
        let mine = match (pull.mine, pull.fix_here) {
            (true, _) => " \u{b7} yours",
            (false, true) => " \u{b7} fixing here",
            (false, false) => "",
        };
        line(
            format!(
                "{by}{} into {} \u{b7} at {}{mine}",
                pull.head_ref,
                pull.base_ref,
                pull.short_head()
            ),
            CheckTone::Note,
        )
    };
    let buttons = (actions.iter().enumerate())
        .filter(|(_, (_, o))| match o {
            PanelOutcome::OpenPr(url) => *url == pull.url,
            PanelOutcome::UpdatePrHead
            | PanelOutcome::FinishReview
            | PanelOutcome::FixHere
            | PanelOutcome::EditPulls => first,
            _ => false,
        })
        .map(|(i, _)| i)
        .collect();
    let name = match feature.pulls().len() > 1 {
        true => {
            let repo = feature
                .repo(&pull.repo)
                .map_or(pull.repo.as_str(), |r| r.name.as_str());
            format!("Pull request #{} \u{b7} {repo}", pull.number)
        }
        false => format!("Pull request #{}", pull.number),
    };
    PageSection::File {
        name,
        state,
        buttons,
    }
}

pub(super) fn line(text: impl Into<String>, tone: CheckTone) -> CheckLine {
    CheckLine {
        text: text.into(),
        tone,
    }
}

/// A labelled section of `lines`.
pub(super) fn lines(label: &str, lines: Vec<CheckLine>) -> PageSection {
    PageSection::Lines {
        label: label.to_owned(),
        lines,
    }
}

/// The button saying where `stage`'s next agent runs, on the machine that
/// `label` says, with machines to place it on.
pub(super) fn place_action(
    label: Option<&str>,
    stage: Stage,
) -> Option<(PageButton, PanelOutcome)> {
    let label = label?.to_owned();
    let style = ButtonStyle::Plain;
    Some((
        PageButton { label, style },
        PanelOutcome::CycleMachine(stage),
    ))
}

/// Whether `stage`'s agent runs on another machine: only there does it need
/// a way to stop that isn't closing its tab, which only ends the view.
pub(super) fn stops_on_machine(feature: &Feature, stage: Stage) -> bool {
    (feature.runs.get(&stage)).is_some_and(|run| run.machine.is_some() && run.pane.is_some())
}

/// The button stopping `stage`'s agent on another machine.
pub(super) fn stop_action(feature: &Feature, stage: Stage) -> Option<(PageButton, PanelOutcome)> {
    stops_on_machine(feature, stage).then(|| {
        let label = "Stop agent".to_owned();
        let style = ButtonStyle::Plain;
        (
            PageButton { label, style },
            PanelOutcome::StopOnMachine(stage),
        )
    })
}

/// The indices in `actions` of what acts on `stage`'s agent: the buttons of
/// its card.
pub(super) fn agent_buttons(stage: Stage, actions: &[(PageButton, PanelOutcome)]) -> Vec<usize> {
    (actions.iter().enumerate())
        .filter(|(_, (_, outcome))| match outcome {
            PanelOutcome::SendPrompt(s)
            | PanelOutcome::ShowAgent(s)
            | PanelOutcome::ResumeAgent(s)
            | PanelOutcome::Continue(s)
            | PanelOutcome::CycleMachine(s)
            | PanelOutcome::StopOnMachine(s)
            | PanelOutcome::StartAgent(s, _) => *s == stage,
            _ => false,
        })
        .map(|(i, _)| i)
        .collect()
}
