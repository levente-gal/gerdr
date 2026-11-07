//! What happened to a feature, told from its record: each save is held up
//! against a `Digest` of the one before, and what changed between them
//! becomes events. Saving is the one place every change passes through.
//!
//! Times come from the record where it keeps them; what it doesn't time
//! is told as of the save. What an event calls for that is too slow for
//! the loop comes back as a `Follow`, for the thread.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use super::{AgentKind, Chat, EndWhy, Event, RunChange, StartHow};
use crate::feature::{
    Feature, Mark, Stage, Status, agents::StageRun, artifacts, fix_round, manual::ManualTest,
    plan_review, review_import, setup::SetupRun, verify,
};

/// What is kept of a record to tell the next save's changes by: small, and
/// quick to take on the loop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Digest {
    /// The history's length.
    pub history: usize,
    /// Its runs by stage and number: each stage's, and Plan review's
    /// reviewer while it is parked.
    pub runs: BTreeMap<(Stage, u32), StageRun>,
    /// The runs whose agent went: one that comes back was resumed.
    pub lost: BTreeSet<(Stage, u32)>,
    pub marks: BTreeMap<Stage, Mark>,
    pub status: Status,
    pub fix_round: Option<fix_round::FixRound>,
    /// The number of the last round over.
    pub last_fix_round: Option<u32>,
    pub fix_rounds: u32,
    /// The AI review read last, and how many findings it had then.
    pub review: Option<(u32, usize)>,
    /// The plan review read last, and how many suggestions it had then.
    pub plan_review: Option<(u32, usize)>,
    pub verify: Option<verify::VerifyRun>,
    /// Its setups: here, and on each other machine.
    pub setups: Vec<SetupRun>,
    pub manual: Option<ManualTest>,
}

/// The digest of `feature`, as when it is first seen: a run without its
/// agent counts as one that lost it.
pub fn digest(feature: &Feature) -> Digest {
    let runs: BTreeMap<_, _> = (runs_of(feature))
        .map(|(stage, run)| ((stage, run.n), run.clone()))
        .collect();
    let lost = (runs.iter())
        .filter(|(_, run)| !run.live())
        .map(|(key, _)| *key)
        .collect();
    Digest {
        history: feature.history.len(),
        runs,
        lost,
        marks: feature.marks.clone(),
        status: feature.status,
        fix_round: feature.fix_round.clone(),
        last_fix_round: feature.last_fix_round.as_ref().map(|r| r.n),
        fix_rounds: feature.fix_rounds,
        review: (feature.review.as_ref()).map(|r| (r.n, r.findings.len())),
        plan_review: (feature.plan_review.as_ref()).map(|r| (r.n, r.suggestions.len())),
        verify: feature.verify.clone(),
        setups: (feature.setup.iter().chain(&feature.setups_elsewhere))
            .cloned()
            .collect(),
        manual: feature.manual.clone(),
    }
}

/// The runs of `feature`, by stage: each stage's, then Plan review's
/// reviewer while the final plan's writer has its stage.
pub fn runs_of(feature: &Feature) -> impl Iterator<Item = (Stage, &StageRun)> {
    let parked = (feature.plan_review.as_ref())
        .and_then(|r| r.reviewer.as_ref())
        .map(|run| (Stage::PlanReview, run));
    (feature.runs.iter())
        .map(|(stage, run)| (*stage, run))
        .chain(parked)
}

/// What a record's save came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diffed {
    /// What happened, in order, each with when the record says it did (ms
    /// since the epoch), if it says.
    pub events: Vec<(Option<u64>, Event)>,
    /// What the thread is to do about it.
    pub follow: Vec<Follow>,
    /// The digest to tell the next save by.
    pub digest: Digest,
}

/// Work an event calls for, too slow for the loop.
#[derive(Clone, Debug, PartialEq)]
pub enum Follow {
    /// Keep `chat` as it is now.
    Capture(Chat),
    /// Keep these files of the feature's data folder, `reason`.
    Outputs { files: Vec<String>, reason: String },
    /// Keep the part of `setup.log` setup `run` wrote.
    SetupSlice { run: Value },
    /// The feature closed: keep the whole record.
    Snapshot,
    /// The chats at work changed.
    Watch,
}

/// What check `repo` `command` is called in the events: `api: npm test`.
pub fn check_key(repo: &str, command: &str) -> String {
    format!("{repo}: {command}")
}

/// What Verify run `run` is called in the events: `v{n}@{started}`, as
/// numbers start again after a start over.
pub fn verify_key(run: &verify::VerifyRun) -> String {
    format!("v{}@{}", run.n, run.started)
}

/// The chat of run `run` of `stage` on `feature`, its id `session` as its
/// command kept it (see `StageRun::session`).
pub fn chat(feature: &Feature, stage: Stage, run: &StageRun, session: Option<String>) -> Chat {
    let (agent, id) = match run.t3_thread() {
        Some(id) => (AgentKind::T3, Some(id.to_owned())),
        None => match AgentKind::of(&run.agent) {
            // The command says it when the name doesn't.
            AgentKind::Unknown => {
                let first = run.base.split_whitespace().next().unwrap_or_default();
                (
                    AgentKind::of(first.rsplit('/').next().unwrap_or(first)),
                    session,
                )
            }
            kind => (kind, session),
        },
    };
    let mut folders = vec![feature.dir.clone()];
    for worktree in feature.repos.iter().map(|r| &r.worktree) {
        if !folders.contains(worktree) {
            folders.push(worktree.clone());
        }
    }
    Chat {
        agent,
        id,
        stage,
        n: run.n,
        started: run.started,
        folders,
        machine: run.machine.clone(),
    }
}

/// What happened between `prev` and `feature`. With no `prev` (the feature
/// is new to this gerdr) nothing is told.
pub fn diff(prev: Option<&Digest>, feature: &Feature) -> Diffed {
    diff_with(prev, feature, &StageRun::session)
}

/// `diff`, each run's chat id read by `session`.
pub fn diff_with(
    prev: Option<&Digest>,
    feature: &Feature,
    session: &dyn Fn(&StageRun) -> Option<String>,
) -> Diffed {
    let mut digest = digest(feature);
    let Some(prev) = prev else {
        return Diffed {
            digest,
            ..Diffed::default()
        };
    };
    let mut out = Out::default();
    let capture = |stage, run: &StageRun| Follow::Capture(chat(feature, stage, run, session(run)));

    // The history, and the start overs it tells of.
    let mut started_over = BTreeSet::new();
    for (i, event) in feature.history.iter().enumerate().skip(prev.history) {
        let at = Some(event.at * 1000);
        out.tell(
            at,
            Event::Note {
                i,
                at: event.at,
                what: event.what.clone(),
            },
        );
        if let Some(stage) = started_over_in(&event.what) {
            started_over.insert(stage);
            out.tell(at, Event::StartedOver { stage: Some(stage) });
        }
    }

    // Where the feature stands.
    let closed =
        feature.status != prev.status && matches!(feature.status, Status::Done | Status::Abandoned);
    if feature.status != prev.status {
        out.tell(
            None,
            Event::Status {
                from: value(&prev.status),
                to: value(&feature.status),
            },
        );
    }

    // The runs gone.
    for (&(stage, n), run) in &prev.runs {
        if digest.runs.contains_key(&(stage, n)) {
            continue;
        }
        let later = (digest.runs.keys()).any(|&(s, m)| s == stage && m > n);
        let why = if started_over.contains(&stage) {
            EndWhy::StartedOver
        } else if later {
            EndWhy::Replaced
        } else {
            EndWhy::Forgotten
        };
        out.tell(
            None,
            Event::RunEnded {
                stage,
                n,
                why,
                run: value(run),
            },
        );
        out.then(capture(stage, run));
        out.then(Follow::Watch);
    }

    // The runs there now: new ones, and what changed of the others.
    for (&(stage, n), run) in &digest.runs {
        let Some(was) = prev.runs.get(&(stage, n)) else {
            let before = (prev.runs.iter())
                .filter(|((s, m), _)| *s == stage && *m < n)
                .map(|(_, r)| r)
                .next_back();
            let how = match before {
                Some(b) if b.stop.is_some() => StartHow::Milestone,
                Some(b) if b.pane.is_some() && b.pane == run.pane => StartHow::Reused,
                Some(b) if session(run).is_some_and(|id| session(b) == Some(id)) => {
                    StartHow::Reused
                }
                _ => StartHow::Fresh,
            };
            out.tell(Some(run.started * 1000), started(stage, run, how));
            out.then(Follow::Watch);
            continue;
        };
        let mut change = |at: Option<u64>, change| {
            out.tell(at, Event::RunChanged { stage, n, change });
        };
        if was.held && !run.held {
            change(None, RunChange::PromptSent);
        }
        if !was.nudged && run.nudged {
            change(None, RunChange::Nudged);
        }
        if run.model.is_some() && was.model != run.model {
            change(None, RunChange::Model);
        }
        if run.sent_back > was.sent_back {
            change(None, RunChange::SentBack);
        }
        let ready = run.ready.filter(|_| was.ready.is_none());
        if let Some(at) = ready {
            change(Some(at * 1000), RunChange::Ready);
        }
        let stopped = run.stop.as_ref().filter(|_| was.stop.is_none());
        if let Some(stop) = stopped {
            change(Some(stop.at * 1000), RunChange::Stop);
        }
        if let Some(blocked) = run.blocked.as_ref().filter(|_| was.blocked.is_none()) {
            change(Some(blocked.at * 1000), RunChange::Blocked);
        }
        if was.blocked.is_some() && run.blocked.is_none() {
            change(None, RunChange::Unblocked);
        }
        let lost = was.live() && !run.live();
        if lost {
            change(None, RunChange::PaneLost);
        }
        let threaded = was.t3_thread().is_none() && run.t3_thread().is_some();
        if threaded {
            change(None, RunChange::T3Thread);
        }
        // Going on: past its stop, or its chat taken up again.
        if was.stop.is_some() && run.stop.is_none() && run.blocked.is_none() {
            out.tell(None, started(stage, run, StartHow::Milestone));
        }
        let back = !was.live() && run.live();
        if back && prev.lost.contains(&(stage, n)) {
            out.tell(None, started(stage, run, StartHow::Resumed));
        }
        if ready.is_some() || stopped.is_some() || lost {
            out.then(capture(stage, run));
        }
        if let Some(file) = ready.and(artifacts::name(stage)) {
            out.then(Follow::Outputs {
                files: vec![file.to_owned()],
                reason: format!("{} run {n} ready", stage.label()),
            });
        }
        if lost || back || threaded {
            out.then(Follow::Watch);
        }
    }
    // A run whose agent went stays lost until it is back.
    digest.lost = (digest.runs.iter())
        .filter(|(key, run)| {
            !run.live() && (prev.lost.contains(key) || prev.runs.get(key).is_some_and(|r| r.live()))
        })
        .map(|(key, _)| *key)
        .collect();

    // Its runs end with it.
    if closed {
        for (&(stage, n), run) in &digest.runs {
            out.tell(
                None,
                Event::RunEnded {
                    stage,
                    n,
                    why: EndWhy::FeatureClosed,
                    run: value(run),
                },
            );
            out.then(capture(stage, run));
        }
        out.then(Follow::Snapshot);
        out.then(Follow::Watch);
    }

    // The marks.
    for (&stage, mark) in &feature.marks {
        if prev.marks.get(&stage) == Some(mark) {
            continue;
        }
        out.tell(
            Some(mark.at * 1000),
            Event::Marked {
                stage,
                mark: value(mark),
            },
        );
        let file = match stage {
            Stage::Verify => Some(verify::EVIDENCE),
            stage => artifacts::name(stage),
        };
        if let Some(file) = file {
            out.then(Follow::Outputs {
                files: vec![file.to_owned()],
                reason: format!("{} marked done", stage.label()),
            });
        }
    }
    for &stage in prev.marks.keys() {
        if !feature.marks.contains_key(&stage) {
            out.tell(None, Event::Unmarked { stage });
        }
    }

    // The fix rounds: one over, then one opened.
    let was_n = prev.fix_round.as_ref().map(|r| r.n);
    let now_n = feature.fix_round.as_ref().map(|r| r.n);
    if let Some(was) = prev.fix_round.as_ref().filter(|_| was_n != now_n) {
        let round = (feature.last_fix_round.as_ref())
            .filter(|r| r.n == was.n)
            .unwrap_or(was);
        out.tell(
            round.ended.map(|at| at * 1000),
            Event::FixRoundClosed {
                n: round.n,
                round: value(round),
            },
        );
        if let Some(run) = feature.runs.get(&Stage::Build) {
            out.then(capture(Stage::Build, run));
        }
        let build = artifacts::name(Stage::Build).unwrap_or("build.md");
        out.then(Follow::Outputs {
            files: vec![fix_round::FILE.to_owned(), build.to_owned()],
            reason: format!("fix round {} over", round.n),
        });
    }
    if let Some(round) = feature.fix_round.as_ref().filter(|_| was_n != now_n) {
        out.tell(
            None,
            Event::FixRoundOpened {
                n: round.n,
                round: value(round),
            },
        );
    }

    // The reviews read.
    let read = digest
        .review
        .filter(|r| prev.review.map(|p| p.0) != Some(r.0));
    if let Some((n, findings)) = read {
        out.tell(
            None,
            Event::ReviewRead {
                n,
                findings,
                review: None,
            },
        );
        out.then(Follow::Outputs {
            files: vec![review_import::REVIEW.to_owned()],
            reason: format!("AI review {n} read"),
        });
    }
    let read = digest
        .plan_review
        .filter(|r| prev.plan_review.map(|p| p.0) != Some(r.0));
    if let Some((n, suggestions)) = read {
        out.tell(
            None,
            Event::PlanReviewRead {
                n,
                suggestions,
                review: None,
            },
        );
        out.then(Follow::Outputs {
            files: vec![plan_review::FILE.to_owned()],
            reason: format!("plan review {n} read"),
        });
    }

    // Verify.
    if let Some(run) = &feature.verify {
        let key = verify_key(run);
        let was = prev.verify.as_ref().filter(|v| verify_key(v) == key);
        if was.is_none() {
            out.tell(
                Some(run.started * 1000),
                Event::VerifyStarted {
                    run: key.clone(),
                    verify: value(run),
                },
            );
        }
        for (i, check) in run.checks.iter().enumerate() {
            // One kept from an earlier run ended in that one.
            let Some(outcome) = check.outcome.as_ref().filter(|_| check.run == run.n) else {
                continue;
            };
            let had = (was.and_then(|v| v.checks.get(i))).is_some_and(|c| c.outcome.is_some());
            if !had {
                out.tell(
                    None,
                    Event::CheckEnded {
                        run: key.clone(),
                        key: check_key(&check.repo, &check.command),
                        i,
                        outcome: value(outcome),
                        log: None,
                        truncated: false,
                    },
                );
            }
        }
        let finished = run
            .finished
            .filter(|_| was.is_none_or(|v| v.finished.is_none()));
        if let Some(at) = finished {
            out.tell(
                Some(at * 1000),
                Event::VerifyEnded {
                    run: key,
                    verify: value(run),
                },
            );
            out.then(Follow::Outputs {
                files: vec![verify::EVIDENCE.to_owned()],
                reason: format!("Verify run {} over", run.n),
            });
        }
    }

    // The setups that ended.
    for setup in &digest.setups {
        if setup.finished.is_none() {
            continue;
        }
        let was =
            (prev.setups.iter()).find(|s| s.machine == setup.machine && s.started == setup.started);
        if was.is_some_and(|s| s.finished.is_some()) {
            continue;
        }
        // The thread tells `SetupEnded` once it has kept the log.
        out.then(Follow::SetupSlice { run: value(setup) });
    }

    // The manual test sheet.
    if digest.manual != prev.manual {
        out.tell(
            None,
            Event::Manual {
                manual: value(&digest.manual),
            },
        );
    }

    Diffed {
        events: out.events,
        follow: out.follow,
        digest,
    }
}

/// What a diff gathers.
#[derive(Default)]
struct Out {
    events: Vec<(Option<u64>, Event)>,
    follow: Vec<Follow>,
}

impl Out {
    fn tell(&mut self, at: Option<u64>, event: Event) {
        self.events.push((at, event));
    }

    /// Ask for `follow`, once.
    fn then(&mut self, follow: Follow) {
        if !self.follow.contains(&follow) {
            self.follow.push(follow);
        }
    }
}

/// `RunStarted` of run `run` of `stage`, begun `how`. Its prompt is kept
/// by whoever writes the event.
fn started(stage: Stage, run: &StageRun, how: StartHow) -> Event {
    Event::RunStarted {
        stage,
        n: run.n,
        run: value(run),
        prompt: None,
        how,
    }
}

/// The stage a history line says started over (see `Feature::start_over`).
fn started_over_in(what: &str) -> Option<Stage> {
    Stage::ALL.into_iter().find(|stage| {
        (what.strip_prefix(stage.label()))
            .and_then(|rest| rest.strip_prefix(" started over"))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
    })
}

/// A piece of the record as JSON. Its types always serialize.
fn value(of: &impl Serialize) -> Value {
    serde_json::to_value(of).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::feature::{
        agents::{Blocked, Stop, T3Run},
        tests::feature,
        verify::{CheckRun, Outcome, VerifyRun},
    };

    /// Run `n` of a stage, in its pane.
    fn run(n: u32) -> StageRun {
        StageRun {
            n,
            agent: "Claude Code".into(),
            base: "claude".into(),
            started: 100 + u64::from(n),
            pane: Some(format!("p{n}")),
            prompt: PathBuf::from(format!("/d/prompts/build-{n}.md")),
            ..StageRun::default()
        }
    }

    /// What changed from `before` to `after`, with no chat ids.
    fn step(before: &Feature, after: &Feature) -> Diffed {
        diff_with(Some(&digest(before)), after, &|_| None)
    }

    fn events(diffed: &Diffed) -> Vec<Event> {
        diffed.events.iter().map(|(_, e)| e.clone()).collect()
    }

    fn changes(diffed: &Diffed) -> Vec<RunChange> {
        (diffed.events.iter())
            .filter_map(|(_, e)| match e {
                Event::RunChanged { change, .. } => Some(*change),
                _ => None,
            })
            .collect()
    }

    fn hows(diffed: &Diffed) -> Vec<(u32, StartHow)> {
        (diffed.events.iter())
            .filter_map(|(_, e)| match e {
                Event::RunStarted { n, how, .. } => Some((*n, *how)),
                _ => None,
            })
            .collect()
    }

    fn ends(diffed: &Diffed) -> Vec<(Stage, u32, EndWhy)> {
        (diffed.events.iter())
            .filter_map(|(_, e)| match e {
                Event::RunEnded { stage, n, why, .. } => Some((*stage, *n, *why)),
                _ => None,
            })
            .collect()
    }

    fn captured(diffed: &Diffed) -> Vec<(Stage, u32)> {
        (diffed.follow.iter())
            .filter_map(|f| match f {
                Follow::Capture(chat) => Some((chat.stage, chat.n)),
                _ => None,
            })
            .collect()
    }

    fn outputs(diffed: &Diffed) -> Vec<String> {
        (diffed.follow.iter())
            .filter_map(|f| match f {
                Follow::Outputs { files, .. } => Some(files.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn a_record_seen_first_tells_nothing() {
        let mut f = feature("a", 1);
        f.log(5, "made");
        f.runs.insert(Stage::Plan, run(1));
        let diffed = diff(None, &f);
        assert!(diffed.events.is_empty());
        assert!(diffed.follow.is_empty());
        assert_eq!(diffed.digest, digest(&f));
        // Seen again unchanged, still nothing.
        let again = diff(Some(&diffed.digest), &f);
        assert!(again.events.is_empty(), "{:?}", again.events);
        assert!(again.follow.is_empty());
    }

    #[test]
    fn each_new_history_line_is_a_note_at_its_time() {
        let mut f = feature("a", 1);
        f.log(5, "made");
        let before = f.clone();
        f.log(7, "Plan ticked");
        f.log(9, "touched api");
        let diffed = step(&before, &f);
        assert_eq!(
            diffed.events,
            [
                (
                    Some(7000),
                    Event::Note {
                        i: 1,
                        at: 7,
                        what: "Plan ticked".into()
                    }
                ),
                (
                    Some(9000),
                    Event::Note {
                        i: 2,
                        at: 9,
                        what: "touched api".into()
                    }
                ),
            ]
        );
    }

    #[test]
    fn a_new_run_starts_fresh_reused_or_after_a_milestone() {
        let f = feature("a", 1);
        // Fresh: nothing before it.
        let mut one = f.clone();
        one.runs.insert(Stage::Build, run(1));
        let diffed = step(&f, &one);
        assert_eq!(hows(&diffed), [(1, StartHow::Fresh)]);
        assert_eq!(diffed.events[0].0, Some(101_000));
        assert!(diffed.follow.contains(&Follow::Watch));
        // Reused: the agent that waited got the new run, in its pane.
        let mut two = one.clone();
        two.runs.insert(
            Stage::Build,
            StageRun {
                pane: Some("p1".into()),
                ..run(2)
            },
        );
        let diffed = step(&one, &two);
        assert_eq!(hows(&diffed), [(2, StartHow::Reused)]);
        assert_eq!(ends(&diffed), [(Stage::Build, 1, EndWhy::Replaced)]);
        // Reused too: its chat id was copied over.
        let mut other = one.clone();
        other.runs.insert(Stage::Build, run(2));
        let same = diff_with(Some(&digest(&one)), &other, &|_| Some("chat".into()));
        assert_eq!(hows(&same), [(2, StartHow::Reused)]);
        let fresh = diff_with(Some(&digest(&one)), &other, &|r| Some(format!("c{}", r.n)));
        assert_eq!(hows(&fresh), [(2, StartHow::Fresh)]);
        // After a milestone stop, a fresh agent per milestone.
        let mut stopped = one.clone();
        stopped.runs.get_mut(&Stage::Build).unwrap().stop = Some(Stop {
            done: 1,
            total: 3,
            at: 50,
        });
        let diffed = step(&stopped, &other);
        assert_eq!(hows(&diffed), [(2, StartHow::Milestone)]);
    }

    #[test]
    fn a_run_going_on_starts_again_without_a_new_number() {
        let mut f = feature("a", 1);
        f.runs.insert(
            Stage::Build,
            StageRun {
                stop: Some(Stop {
                    done: 1,
                    total: 2,
                    at: 50,
                }),
                ..run(1)
            },
        );
        // Let go on past its stop.
        let mut on = f.clone();
        on.runs.get_mut(&Stage::Build).unwrap().stop = None;
        assert_eq!(hows(&step(&f, &on)), [(1, StartHow::Milestone)]);
        // Stopped for being blocked instead: no start.
        let mut blocked = on.clone();
        blocked.runs.get_mut(&Stage::Build).unwrap().blocked = Some(Blocked {
            why: "no".into(),
            at: 60,
        });
        assert_eq!(hows(&step(&f, &blocked)), []);

        // Its tab closed, then its chat resumed.
        let mut lost = on.clone();
        lost.runs.get_mut(&Stage::Build).unwrap().pane = None;
        let gone = step(&on, &lost);
        assert_eq!(changes(&gone), [RunChange::PaneLost]);
        assert_eq!(captured(&gone), [(Stage::Build, 1)]);
        let back = diff_with(Some(&gone.digest), &on, &|_| None);
        assert_eq!(hows(&back), [(1, StartHow::Resumed)]);
        assert!(back.digest.lost.is_empty());
        // One on a machine getting its first pane wasn't resumed.
        let mut waits = feature("a", 1);
        waits.runs.insert(
            Stage::Build,
            StageRun {
                pane: None,
                ..run(1)
            },
        );
        let started = diff_with(Some(&Digest::default()), &waits, &|_| None);
        assert_eq!(hows(&started), [(1, StartHow::Fresh)]);
        let mut placed = waits.clone();
        placed.runs.get_mut(&Stage::Build).unwrap().pane = Some("m/p1".into());
        assert_eq!(
            hows(&diff_with(Some(&started.digest), &placed, &|_| None)),
            []
        );
    }

    #[test]
    fn each_change_of_a_run_is_told() {
        let mut f = feature("a", 1);
        f.runs.insert(
            Stage::Plan,
            StageRun {
                held: true,
                ..run(1)
            },
        );
        let edit = |change: &dyn Fn(&mut StageRun)| {
            let mut after = f.clone();
            change(after.runs.get_mut(&Stage::Plan).unwrap());
            step(&f, &after)
        };
        let told = |change: &dyn Fn(&mut StageRun)| changes(&edit(change));
        assert_eq!(told(&|r| r.held = false), [RunChange::PromptSent]);
        assert_eq!(told(&|r| r.nudged = true), [RunChange::Nudged]);
        assert_eq!(told(&|r| r.model = Some("opus".into())), [RunChange::Model]);
        assert_eq!(told(&|r| r.sent_back = 1), [RunChange::SentBack]);
        assert_eq!(
            told(&|r| r.t3 = Some(T3Run::Thread {
                id: "t".into(),
                agent: None
            })),
            [RunChange::T3Thread]
        );
        let ready = edit(&|r| r.ready = Some(9));
        assert_eq!(
            ready.events,
            [(
                Some(9000),
                Event::RunChanged {
                    stage: Stage::Plan,
                    n: 1,
                    change: RunChange::Ready
                }
            )]
        );
        assert_eq!(captured(&ready), [(Stage::Plan, 1)]);
        assert_eq!(outputs(&ready), ["plan.md"]);
        let stop = edit(&|r| {
            r.stop = Some(Stop {
                done: 1,
                total: 2,
                at: 8,
            })
        });
        assert_eq!(changes(&stop), [RunChange::Stop]);
        assert_eq!(stop.events[0].0, Some(8000));
        assert_eq!(captured(&stop), [(Stage::Plan, 1)]);
        assert_eq!(
            told(&|r| {
                r.blocked = Some(Blocked {
                    why: "x".into(),
                    at: 3,
                })
            }),
            [RunChange::Blocked]
        );
        let mut blocked = f.clone();
        blocked.runs.get_mut(&Stage::Plan).unwrap().blocked = Some(Blocked {
            why: "x".into(),
            at: 3,
        });
        assert_eq!(changes(&step(&blocked, &f)), [RunChange::Unblocked]);
        assert_eq!(told(&|r| r.pane = None), [RunChange::PaneLost]);
        // What isn't watched says nothing.
        assert_eq!(told(&|r| r.command = "other".into()), []);
    }

    #[test]
    fn a_run_ends_replaced_forgotten_started_over_or_closed() {
        let mut f = feature("a", 1);
        f.runs.insert(Stage::Plan, run(1));
        // Forgotten, its number kept.
        let mut forgot = f.clone();
        forgot.forget_run(Stage::Plan);
        let diffed = step(&f, &forgot);
        assert_eq!(ends(&diffed), [(Stage::Plan, 1, EndWhy::Forgotten)]);
        assert_eq!(captured(&diffed), [(Stage::Plan, 1)]);
        // Replaced by a later run.
        let mut next = f.clone();
        next.runs.insert(Stage::Plan, run(2));
        assert_eq!(ends(&step(&f, &next)), [(Stage::Plan, 1, EndWhy::Replaced)]);
        // The stage started over.
        let mut at_plan = f.clone();
        at_plan.marks.insert(
            Stage::Discover,
            Mark {
                at: 1,
                by: "you".into(),
                note: String::new(),
                patch: None,
            },
        );
        let mut over = at_plan.clone();
        assert!(over.start_over(Stage::Plan, 5));
        let diffed = step(&at_plan, &over);
        assert_eq!(ends(&diffed), [(Stage::Plan, 1, EndWhy::StartedOver)]);
        assert!(events(&diffed).contains(&Event::StartedOver {
            stage: Some(Stage::Plan)
        }));
        // The feature closed: what runs ends, and the record is kept.
        let mut closed = f.clone();
        closed.status = Status::Abandoned;
        let diffed = step(&f, &closed);
        assert_eq!(ends(&diffed), [(Stage::Plan, 1, EndWhy::FeatureClosed)]);
        assert!(events(&diffed).contains(&Event::Status {
            from: json!("open"),
            to: json!("abandoned")
        }));
        assert!(diffed.follow.contains(&Follow::Snapshot));
        assert_eq!(captured(&diffed), [(Stage::Plan, 1)]);
    }

    #[test]
    fn a_parked_reviewer_is_still_its_run() {
        let mut f = feature("a", 1);
        f.runs.insert(Stage::PlanReview, run(1));
        f.plan_review = Some(serde_json::from_value(json!({"n": 1})).unwrap());
        // The writer takes the stage, the reviewer parked.
        let mut parked = f.clone();
        let reviewer = parked.runs.remove(&Stage::PlanReview);
        parked.plan_review.as_mut().unwrap().reviewer = reviewer;
        parked.runs.insert(Stage::PlanReview, run(2));
        let diffed = step(&f, &parked);
        assert_eq!(ends(&diffed), []);
        assert_eq!(hows(&diffed), [(2, StartHow::Fresh)]);
    }

    #[test]
    fn marks_and_their_files() {
        let mut f = feature("a", 1);
        let before = f.clone();
        let mark = Mark {
            at: 4,
            by: "you".into(),
            note: String::new(),
            patch: None,
        };
        f.marks.insert(Stage::Discover, mark.clone());
        let diffed = step(&before, &f);
        assert_eq!(
            diffed.events,
            [(
                Some(4000),
                Event::Marked {
                    stage: Stage::Discover,
                    mark: value(&mark)
                }
            )]
        );
        assert_eq!(outputs(&diffed), ["brief.md"]);
        let mut verified = f.clone();
        verified.marks.insert(Stage::Verify, mark);
        assert_eq!(outputs(&step(&f, &verified)), [verify::EVIDENCE]);
        assert_eq!(
            events(&step(&f, &before)),
            [Event::Unmarked {
                stage: Stage::Discover
            }]
        );
    }

    #[test]
    fn fix_rounds_open_and_close() {
        let round = |n: u32, ended: Option<u64>| -> fix_round::FixRound {
            serde_json::from_value(json!({
                "n": n, "what": "2 findings", "input": "/i", "stage": "ai-review", "ended": ended
            }))
            .unwrap()
        };
        let mut f = feature("a", 1);
        f.runs.insert(Stage::Build, run(1));
        let mut open = f.clone();
        open.fix_round = Some(round(1, None));
        open.fix_rounds = 1;
        let diffed = step(&f, &open);
        assert_eq!(
            events(&diffed),
            [Event::FixRoundOpened {
                n: 1,
                round: value(&round(1, None))
            }]
        );
        // Over: it moves to the last round, ended.
        let mut over = open.clone();
        over.fix_round = None;
        over.last_fix_round = Some(round(1, Some(30)));
        let diffed = step(&open, &over);
        assert_eq!(
            diffed.events,
            [(
                Some(30_000),
                Event::FixRoundClosed {
                    n: 1,
                    round: value(&round(1, Some(30)))
                }
            )]
        );
        assert_eq!(captured(&diffed), [(Stage::Build, 1)]);
        assert_eq!(outputs(&diffed), [fix_round::FILE, "build.md"]);
        // Sent again, it is open once more.
        let diffed = step(&over, &open);
        assert!(matches!(
            events(&diffed)[..],
            [Event::FixRoundOpened { n: 1, .. }]
        ));
        // One over and the next opened in one save.
        let mut next = open.clone();
        next.fix_round = Some(round(2, None));
        let told: Vec<_> = (events(&step(&open, &next)).into_iter())
            .map(|e| match e {
                Event::FixRoundOpened { n, .. } => ("opened", n),
                Event::FixRoundClosed { n, .. } => ("closed", n),
                _ => ("other", 0),
            })
            .collect();
        assert_eq!(told, [("closed", 1), ("opened", 2)]);
    }

    #[test]
    fn reviews_read() {
        let f = feature("a", 1);
        let mut read = f.clone();
        read.review = Some(serde_json::from_value(json!({"n": 1})).unwrap());
        let diffed = step(&f, &read);
        assert_eq!(
            events(&diffed),
            [Event::ReviewRead {
                n: 1,
                findings: 0,
                review: None
            }]
        );
        assert_eq!(outputs(&diffed), [review_import::REVIEW]);
        let mut again = read.clone();
        again.review.as_mut().unwrap().n = 2;
        assert!(matches!(
            events(&step(&read, &again))[..],
            [Event::ReviewRead { n: 2, .. }]
        ));
        let mut plan = f.clone();
        plan.plan_review = Some(serde_json::from_value(json!({"n": 1})).unwrap());
        let diffed = step(&f, &plan);
        assert_eq!(
            events(&diffed),
            [Event::PlanReviewRead {
                n: 1,
                suggestions: 0,
                review: None
            }]
        );
        assert_eq!(outputs(&diffed), [plan_review::FILE]);
    }

    #[test]
    fn a_verify_run_starts_its_checks_end_and_it_ends() {
        let check = |run: u32, outcome: Option<Outcome>| CheckRun {
            repo: "api".into(),
            command: "npm test".into(),
            run,
            outcome,
            ..CheckRun::default()
        };
        let passed = Outcome {
            repo: "api".into(),
            command: "npm test".into(),
            exit: Some(0),
            ..Outcome::default()
        };
        let f = feature("a", 1);
        let mut started = f.clone();
        started.verify = Some(VerifyRun {
            n: 2,
            started: 40,
            // One kept from run 1, one to run now.
            checks: vec![check(1, Some(passed.clone())), check(2, None)],
            ..VerifyRun::default()
        });
        let diffed = step(&f, &started);
        assert!(matches!(
            &diffed.events[..],
            [(Some(40_000), Event::VerifyStarted { run, .. })] if run == "v2@40"
        ));
        let mut ended = started.clone();
        let v = ended.verify.as_mut().unwrap();
        v.checks[1].outcome = Some(passed.clone());
        let diffed = step(&started, &ended);
        assert_eq!(
            events(&diffed),
            [Event::CheckEnded {
                run: "v2@40".into(),
                key: "api: npm test".into(),
                i: 1,
                outcome: value(&passed),
                log: None,
                truncated: false
            }]
        );
        let mut finished = ended.clone();
        finished.verify.as_mut().unwrap().finished = Some(70);
        let diffed = step(&ended, &finished);
        assert!(matches!(
            &diffed.events[..],
            [(Some(70_000), Event::VerifyEnded { run, .. })] if run == "v2@40"
        ));
        assert_eq!(outputs(&diffed), [verify::EVIDENCE]);
        // After a start over the number comes again, at another time.
        let mut anew = finished.clone();
        let v = anew.verify.as_mut().unwrap();
        (v.started, v.finished, v.checks) = (90, None, Vec::new());
        assert!(matches!(
            &events(&step(&finished, &anew))[..],
            [Event::VerifyStarted { run, .. }] if run == "v2@90"
        ));
    }

    #[test]
    fn a_setup_that_ends_has_its_log_kept() {
        let mut f = feature("a", 1);
        f.setup = Some(SetupRun {
            started: 10,
            ..SetupRun::default()
        });
        let mut ended = f.clone();
        ended.setup.as_mut().unwrap().finished = Some(20);
        let diffed = step(&f, &ended);
        let run = value(ended.setup.as_ref().unwrap());
        // The thread tells it, with the log it kept.
        assert_eq!(diffed.events, []);
        assert_eq!(diffed.follow, [Follow::SetupSlice { run }]);
        // Moved aside by a setup on another machine, it doesn't end again.
        let mut elsewhere = ended.clone();
        elsewhere.begin_setup(SetupRun {
            machine: Some("m".into()),
            started: 30,
            ..SetupRun::default()
        });
        assert_eq!(step(&ended, &elsewhere).follow, []);
    }

    #[test]
    fn the_manual_sheet_changing_is_told() {
        let f = feature("a", 1);
        let mut tried = f.clone();
        tried.manual = Some(ManualTest {
            patch: "p".into(),
            checks: Vec::new(),
        });
        assert_eq!(
            events(&step(&f, &tried)),
            [Event::Manual {
                manual: json!({"patch": "p", "checks": []})
            }]
        );
    }

    #[test]
    fn a_chat_is_named_by_its_agent_and_folders() {
        let mut f = feature("a", 1);
        f.repos.push(crate::feature::Repo {
            name: "api".into(),
            main: "/r/api".into(),
            base: "main".into(),
            base_commit: "c".into(),
            worktree: "/r/.features/a/api".into(),
            touched: true,
            branch: None,
            project: None,
        });
        let mut r = run(3);
        r.machine = Some("m".into());
        let chat = chat(&f, Stage::Build, &r, Some("id".into()));
        assert_eq!(
            chat,
            Chat {
                agent: AgentKind::Claude,
                id: Some("id".into()),
                stage: Stage::Build,
                n: 3,
                started: 103,
                folders: vec![f.dir.clone(), "/r/.features/a/api".into()],
                machine: Some("m".into()),
            }
        );
        // Named oddly, the command tells it.
        r.agent = "My agent".into();
        r.base = "/usr/bin/codex --yolo".into();
        assert_eq!(
            super::chat(&f, Stage::Build, &r, None).agent,
            AgentKind::Codex
        );
        // In T3 Code, its thread.
        r.t3 = Some(T3Run::Thread {
            id: "t9".into(),
            agent: None,
        });
        let chat = super::chat(&f, Stage::Build, &r, Some("id".into()));
        assert_eq!((chat.agent, chat.id), (AgentKind::T3, Some("t9".into())));
    }
}
