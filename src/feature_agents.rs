//! The agent stages of features, run by gerdr: start a stage's agent on
//! its rendered prompt, watch it until it is done and its file is good
//! (nudging it once when it isn't), then wait at the gate for the user to
//! approve the file or send it back with comments (see `doc_review`).
//!
//! The watch runs after each read of herdr's agents, over the shown
//! project's features (the others' panes are in sessions gerdr doesn't
//! read). An agent is done once herdr saw it working and then idle or
//! done; one herdr doesn't recognise is done once its file stays good and
//! unchanged for `STABLE`. Its file only counts when written since the run
//! started (or was sent back). A Plan agent's run is ready only once the
//! code reads unchanged: no uncommitted changes and no commits.
//!
//! A Build agent has no review gate: its `build.md` may be older than the
//! run (a fresh agent goes on from it), it stops after each milestone (the
//! milestone gate, `continue_stage_agent`), and once every milestone is
//! ticked gerdr reads the code and marks Build done itself (see
//! `feature_build`).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

use crate::{
    Gerdr,
    agent_icons::AgentIcon,
    agents,
    chrome::AgentChip,
    cli_agents::CliAgent,
    feature::{
        self, Feature, PerProject, Stage, Status,
        agents::{Flavor, StageRun, Stop, T3Run, command_line, needs, resume_line, session_file},
        artifacts, graph, layout,
        patch::Reading,
        review_import::FindingState,
        store, verify,
    },
    herdr::AgentStatus,
    projects,
    projects::Workflow,
    prompts::{self, render::Vars},
};

/// A gate waiting for the user: an agent stage's file is ready to review,
/// a Build agent stopped after a milestone, Verify failed more often than
/// gerdr sends to the Build agent, AI review's findings wait to be decided
/// or sent, or Code review waits for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    pub project: String,
    pub slug: String,
    /// The feature's title.
    pub title: String,
    pub stage: Stage,
    /// When it got ready (unix seconds).
    pub since: u64,
    pub kind: GateKind,
}

/// What a gate waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GateKind {
    /// The stage's file is ready: Approve or Send back.
    #[default]
    Review,
    /// The Build agent ticked `done` of `total` milestones and waits:
    /// Continue.
    Milestone { done: u32, total: u32 },
    /// Verify failed `fixes` times in a row; gerdr stopped sending the
    /// failures to the Build agent (stage: Verify).
    VerifyFailed { fixes: u32 },
    /// AI review found `open` findings to decide (Fix or Dismiss), and
    /// `fix` the user wants fixed, not sent to Build yet (stage: AI
    /// review).
    Findings { open: u32, fix: u32 },
    /// Code review waits for the user to review the diff.
    CodeReview,
    /// Manual test waits for the user to try the brief's criteria:
    /// `passed` of `total` passed on the code as it is.
    ManualTest { passed: u32, total: u32 },
}

/// How long a file must stay good and unchanged to count as done when
/// herdr can't say what the agent does, and how long a pane must be gone
/// before its run forgets it.
const STABLE: Duration = Duration::from_secs(10);
/// How often a refused Plan's or review's code is read again: undoing the agent's
/// changes lets the plan go up for review.
const RECHECK: Duration = Duration::from_secs(10);

/// A file's modification time (unix seconds) and length: whether it
/// changed.
pub(crate) type Stamp = (u64, u64);

/// What gerdr keeps of a stage agent's run while it watches it; not
/// saved: a run seen first after a restart starts afresh.
#[derive(Clone, Debug, Default)]
pub(crate) struct Watch {
    /// The run watched: a new one starts the watch again.
    pub(crate) n: u32,
    /// herdr saw it working since it started or was sent back.
    pub(crate) worked: bool,
    /// Unix seconds its file must be written at or after to count.
    pub(crate) after: u64,
    /// Its file's stamp, good, and since when unchanged.
    stable: Option<(Stamp, Instant)>,
    /// Since when its pane is gone.
    gone: Option<Instant>,
    /// What its file lacks, as last read.
    pub(crate) missing: Vec<String>,
    /// The code is being read for a Plan, Build or review file with this
    /// stamp.
    pub(crate) checking: Option<Stamp>,
    /// The Plan, Build or review file with this stamp was refused: the code
    /// isn't as it should be, as the text says ("uncommitted changes in
    /// api").
    pub(crate) refused: Option<(Stamp, String)>,
    /// When the code was last read for the refused file.
    pub(crate) refused_at: Option<Instant>,
    /// The Build agent was told to go on after this many milestones. A
    /// build log still at that many is the stop it went on from: when
    /// herdr can't say whether the agent rests, it isn't a stop again.
    pub(crate) went_on: Option<u32>,
}

impl Watch {
    /// A watch of run `n` from its start: it waits for the agent to work,
    /// and any file counts.
    pub(crate) fn of(n: u32) -> Self {
        Self {
            n,
            ..Self::default()
        }
    }
}

/// The watch's key: project, slug and stage.
pub(crate) type WatchKey = (String, String, Stage);

/// The stage's file as the watch found it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum File {
    Absent,
    /// Written before the run started: not its work.
    Old,
    Bad(Stamp, Vec<String>),
    /// A build log with `done` of `total` milestones ticked, some but not
    /// all: the Build agent stops there.
    Partial(Stamp, u32, u32),
    Good(Stamp),
}

impl File {
    /// Its stamp, when it counts as the run's work so far.
    fn finished(&self) -> Option<Stamp> {
        match self {
            Self::Good(stamp) | Self::Partial(stamp, ..) => Some(*stamp),
            _ => None,
        }
    }

    /// What a done agent with this file comes to, if it is enough.
    fn verdict(&self) -> Option<Verdict> {
        match self {
            Self::Good(stamp) => Some(Verdict::Ready(*stamp)),
            Self::Partial(_, done, total) => Some(Verdict::Stop {
                done: *done,
                total: *total,
            }),
            _ => None,
        }
    }
}

/// What the watch makes of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Wait,
    /// Done with a good file (this stamp).
    Ready(Stamp),
    /// Done without a good file: tell it once. `absent`: there is none.
    Nudge {
        absent: bool,
    },
    /// Its pane is gone, and with it the agent.
    Gone,
    /// A Build agent stopped after milestone `done` of `total`.
    Stop {
        done: u32,
        total: u32,
    },
}

/// Judge a run: `status` is herdr's for its pane (`None`: herdr doesn't
/// list it), `alive` whether its pane is still there.
fn judge(
    watch: &mut Watch,
    status: Option<AgentStatus>,
    alive: bool,
    file: &File,
    discover: bool,
    nudged: bool,
    now: Instant,
) -> Verdict {
    match file {
        File::Bad(_, lacks) => watch.missing = lacks.clone(),
        _ => watch.missing.clear(),
    }
    if file.finished().is_none() {
        watch.stable = None;
    }
    if matches!(status, Some(AgentStatus::Working | AgentStatus::Blocked)) {
        watch.worked = true;
        watch.stable = None;
        watch.gone = None;
        return Verdict::Wait;
    }
    if !alive {
        let since = *watch.gone.get_or_insert(now);
        if now.duration_since(since) < STABLE {
            return Verdict::Wait;
        }
        return file.verdict().unwrap_or(Verdict::Gone);
    }
    watch.gone = None;
    let done = matches!(status, Some(AgentStatus::Done | AgentStatus::Idle)) && watch.worked;
    if done {
        if let Some(verdict) = file.verdict() {
            return verdict;
        }
        return match file {
            File::Good(_) | File::Partial(..) => Verdict::Wait,
            // A chat: the brief comes when the user and it agree.
            File::Absent | File::Old if discover => Verdict::Wait,
            _ if nudged => Verdict::Wait,
            File::Absent | File::Old => Verdict::Nudge { absent: true },
            File::Bad(..) => Verdict::Nudge { absent: false },
        };
    }
    let went_on = match file {
        File::Partial(_, done, _) => watch.went_on.is_some_and(|after| *done <= after),
        _ => false,
    };
    if matches!(status, None | Some(AgentStatus::Unknown))
        && !went_on
        && let Some(stamp) = file.finished()
    {
        match watch.stable {
            Some((seen, since)) if seen == stamp => {
                if now.duration_since(since) >= STABLE {
                    return file.verdict().unwrap_or(Verdict::Wait);
                }
            }
            _ => watch.stable = Some((stamp, now)),
        }
    }
    Verdict::Wait
}

/// Whether `text` is a good `stage` file for `feature`; else what it
/// lacks. A multi-repository plan must name the touched repositories.
pub(crate) fn validate(feature: &Feature, stage: Stage, text: &str) -> Result<(), Vec<String>> {
    let mut lacks = artifacts::check(stage, text).err().unwrap_or_default();
    if stage == Stage::Plan && feature.many_repos() {
        // By plain name: a plan's `web:api` names `api` too.
        let mut touched: Vec<&str> = feature.touched().map(|r| r.name.as_str()).collect();
        touched.sort_unstable();
        touched.dedup();
        for repo in artifacts::unnamed_repos(text, &touched) {
            lacks.push(format!("Repositories doesn't name {repo}"));
        }
    }
    if lacks.is_empty() { Ok(()) } else { Err(lacks) }
}

/// Whether `stage` of `feature` can be approved with its file reading
/// `text` (`None`: there is none); else why not.
pub(crate) fn approval(feature: &Feature, stage: Stage, text: Option<&str>) -> Result<(), String> {
    let title = &feature.title;
    if feature.status != Status::Open {
        return Err(format!("\u{201c}{title}\u{201d} isn't open"));
    }
    if feature.is_done(stage) {
        return Err(format!("{} is approved already", stage.label()));
    }
    if feature.current() != Some(stage) {
        let first = feature.current().map_or("", Stage::label);
        return Err(format!("Mark {first} done first"));
    }
    // These pass by their own checks, not by their file alone.
    match stage {
        Stage::AiReview => return Err("AI review is done once its findings are decided".into()),
        Stage::CodeReview => return Err("Approve Code review from the feature's diff".into()),
        Stage::ManualTest => return Err("Mark Manual test passed from its sheet".into()),
        _ => {}
    }
    let name = artifacts::name(stage).unwrap_or("its file");
    let Some(text) = text else {
        return Err(format!("There's no {name} yet"));
    };
    validate(feature, stage, text)
        .map_err(|lacks| format!("{name} isn't ready: {}", lacks.join("; ")))
}

/// The gate `f` waits at, if any: its stage, since when, and what for.
/// A file to review (Discover, Plan), a Build agent stopped after a
/// milestone, or Verify failed once gerdr stopped sending its failures
/// to the Build agent; AI review's findings once read in, while any wait
/// to be decided or sent to Build; Code review, from AI review's mark. A
/// Build run's `ready` says Build was marked done, not that anything waits
/// for review.
/// Manual test, while its brief has criteria (`manual` says how many
/// passed of how many), from Code review's mark.
pub(crate) fn gate_of(
    f: &Feature,
    manual: impl Fn(&Feature) -> Option<(u32, u32)>,
) -> Option<(Stage, u64, GateKind)> {
    if f.status != Status::Open {
        return None;
    }
    let stage = f.current()?;
    match stage {
        Stage::Discover | Stage::Plan => {
            let since = f.runs.get(&stage)?.ready?;
            Some((stage, since, GateKind::Review))
        }
        Stage::Build => {
            let stop = f.runs.get(&stage)?.stop?;
            let kind = GateKind::Milestone {
                done: stop.done,
                total: stop.total,
            };
            Some((stage, stop.at, kind))
        }
        Stage::Verify => {
            let run = f.verify.as_ref()?;
            let failed = run.passed == Some(false) && run.fixes >= verify::MAX_FIXES;
            let since = run.finished.filter(|_| failed)?;
            Some((stage, since, GateKind::VerifyFailed { fixes: run.fixes }))
        }
        Stage::AiReview => {
            let since = f.runs.get(&stage)?.ready?;
            let review = f.review.as_ref()?;
            let count = |state| u32::try_from(review.count(state)).unwrap_or(u32::MAX);
            let open = count(FindingState::Open);
            let fix = count(FindingState::Fix);
            (open > 0 || fix > 0).then_some((stage, since, GateKind::Findings { open, fix }))
        }
        Stage::CodeReview => {
            let since = f.marks.get(&Stage::AiReview).map_or(f.created, |m| m.at);
            Some((stage, since, GateKind::CodeReview))
        }
        Stage::ManualTest => {
            let (passed, total) = manual(f).filter(|(_, total)| *total > 0)?;
            let since = f.marks.get(&Stage::CodeReview).map_or(f.created, |m| m.at);
            Some((stage, since, GateKind::ManualTest { passed, total }))
        }
        _ => None,
    }
}

/// The gates waiting in `all` (features by project), oldest first;
/// `manual` says how far a feature's manual test went (see `gate_of`).
fn gates(
    all: &BTreeMap<String, Vec<Feature>>,
    manual: impl Fn(&Feature) -> Option<(u32, u32)> + Copy,
) -> Vec<Gate> {
    let mut gates: Vec<Gate> = all
        .iter()
        .flat_map(|(project, features)| {
            features.iter().filter_map(move |f| {
                let (stage, since, kind) = gate_of(f, manual)?;
                Some(Gate {
                    project: project.clone(),
                    slug: f.slug.clone(),
                    title: f.title.clone(),
                    stage,
                    since,
                    kind,
                })
            })
        })
        .collect();
    gates.sort_by(|a, b| (a.since, &a.project, &a.slug).cmp(&(b.since, &b.project, &b.slug)));
    gates
}

/// The agent to run a stage with: `configured` (a picker name, or else a
/// command) if set, else the first installed CLI agent with a command.
/// Its name and command.
fn pick_agent(configured: Option<&str>, installed: &[CliAgent]) -> Option<(String, String)> {
    let configured = configured.map(str::trim).filter(|c| !c.is_empty());
    if let Some(want) = configured {
        let named = installed
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(want))
            .map(|a| (a.name.clone(), a.stage_base().to_owned()));
        return named.or_else(|| {
            let program = want.split_whitespace().next().unwrap_or(want);
            let name = program.rsplit('/').next().unwrap_or(program);
            Some((name.to_owned(), want.to_owned()))
        });
    }
    installed
        .iter()
        .find_map(|a| Some((a.name.clone(), a.command()?.to_owned())))
}

/// Who reviews the code at AI review.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pick {
    /// This agent: its name and command.
    Agent(String, String),
    /// No CLI agent is installed at all.
    NoCli,
    /// None that may: why not, for a warning.
    Refused(String),
}

/// Where the user picks AI review's agent, or lets Build's review itself.
const REVIEWER_SETTING: &str = "pick another agent on AI review's page, or add \"allow_same_reviewer\": true to the \"workflow\" of the project's file";

/// The program a command line runs, without its folder: `claude` for
/// `/usr/bin/claude --yes`.
fn program(command: &str) -> &str {
    let first = command.split_whitespace().next().unwrap_or("");
    first.rsplit('/').next().unwrap_or(first)
}

/// Whether `name`, run by `command`, is `builder` (an agent's name and
/// command): the same name, or the same program.
fn same_agent(name: &str, command: &str, builder: Option<(&str, &str)>) -> bool {
    // T3 Code runs whichever agent the user picks in its draft: told apart
    // once its thread runs (see `Gerdr::t3_stage_thread_sent`).
    if Flavor::of(command) == Flavor::T3 {
        return false;
    }
    builder.is_some_and(|(b_name, b_command)| {
        let (prog, b_prog) = (program(command), program(b_command));
        b_name.eq_ignore_ascii_case(name) || (!prog.is_empty() && prog.eq_ignore_ascii_case(b_prog))
    })
}

/// The agent to review the code built by `builder` (Build's agent: its
/// name and command): `configured` if set, else the first installed CLI
/// agent other than the builder. The same agent is the same name, or the
/// same program, so `claude` set as a command is Claude Code. The builder
/// reviewing its own code is refused unless `allow_same`: a model misses
/// what it missed.
fn pick_reviewer(
    configured: Option<&str>,
    builder: Option<(&str, &str)>,
    installed: &[CliAgent],
    allow_same: bool,
) -> Pick {
    let same = |name: &str, command: &str| same_agent(name, command, builder);
    let configured = configured.map(str::trim).filter(|c| !c.is_empty());
    if configured.is_some() {
        let Some((name, command)) = pick_agent(configured, installed) else {
            return Pick::NoCli;
        };
        if same(&name, &command) && !allow_same {
            return Pick::Refused(format!(
                "AI review is set to {name}, which built the code; {REVIEWER_SETTING}"
            ));
        }
        return Pick::Agent(name, command);
    }
    let other = installed
        .iter()
        .filter_map(|a| Some((a.name.clone(), a.command()?.to_owned())))
        .find(|(name, command)| !same(name, command));
    if let Some((name, command)) = other {
        return Pick::Agent(name, command);
    }
    match pick_agent(None, installed) {
        None => Pick::NoCli,
        Some((name, command)) if allow_same => Pick::Agent(name, command),
        Some((name, _)) => Pick::Refused(format!(
            "No agent but {name}, which built the code, is installed to review it; install another, or {REVIEWER_SETTING}"
        )),
    }
}

/// Who built `feature`'s code, for AI review to differ from: Build's last
/// run, else the agent Build will start. Build marked done by hand has none.
fn builder_of(
    feature: &Feature,
    workflow: Option<&Workflow>,
    installed: &[CliAgent],
) -> Option<(String, String)> {
    if let Some(run) = feature.runs.get(&Stage::Build) {
        return Some(match &run.t3 {
            Some(T3Run::Thread {
                agent: Some(agent), ..
            }) => (agent.clone(), t3_agent_program(agent).to_owned()),
            _ => (run.agent.clone(), run.command.clone()),
        });
    }
    if feature.is_done(Stage::Build) {
        return None;
    }
    match stage_pick(feature, Stage::Build, workflow, installed) {
        Pick::Agent(name, command) => Some((name, command)),
        _ => None,
    }
}

/// The agent `stage` starts when nothing is picked on its page: the
/// project's `workflow.agents`, else the first installed that may.
fn default_pick(
    feature: &Feature,
    stage: Stage,
    workflow: Option<&Workflow>,
    installed: &[CliAgent],
) -> Pick {
    let set = workflow
        .and_then(|w| w.agents.get(&stage))
        .map(String::as_str);
    resolve_pick(set, feature, stage, workflow, installed)
}

/// The agent `configured` (a name or a command, if any) gives `stage`.
fn resolve_pick(
    configured: Option<&str>,
    feature: &Feature,
    stage: Stage,
    workflow: Option<&Workflow>,
    installed: &[CliAgent],
) -> Pick {
    if stage == Stage::AiReview {
        let builder = builder_of(feature, workflow, installed);
        let builder = builder.as_ref().map(|(n, c)| (n.as_str(), c.as_str()));
        let allow_same = workflow.is_some_and(|w| w.allow_same_reviewer);
        return pick_reviewer(configured, builder, installed, allow_same);
    }
    pick_agent(configured, installed)
        .map_or(Pick::NoCli, |(name, command)| Pick::Agent(name, command))
}

/// The agent `stage` of `feature` starts: the one picked on its page, else
/// `default_pick`. Its page and the start both ask here, so the chip lit
/// is the agent that runs. A picked agent since uninstalled is refused: its
/// name is no command to run.
fn stage_pick(
    feature: &Feature,
    stage: Stage,
    workflow: Option<&Workflow>,
    installed: &[CliAgent],
) -> Pick {
    let Some(picked) = feature.stage_agents.get(&stage) else {
        return default_pick(feature, stage, workflow, installed);
    };
    let there = |a: &CliAgent| a.name.eq_ignore_ascii_case(picked);
    if !installed.iter().any(there) {
        return Pick::Refused(format!(
            "{picked}, picked for {}, isn't installed: pick another agent on its page",
            stage.label()
        ));
    }
    resolve_pick(Some(picked), feature, stage, workflow, installed)
}

/// The chips on `stage`'s page to pick its agent with: the installed
/// agents stages are offered, and whatever the project's file gives it
/// besides. None on a stage no agent runs, one that is done, or a feature
/// that is closed.
pub(crate) fn stage_chips(
    feature: &Feature,
    stage: Stage,
    workflow: Option<&Workflow>,
    installed: &[CliAgent],
) -> Vec<AgentChip> {
    if feature.status != Status::Open || needs(stage).is_none() || feature.is_done(stage) {
        return Vec::new();
    }
    let mut agents: Vec<(String, String)> = installed
        .iter()
        .filter_map(|a| {
            let base = a.stage_base();
            matches!(
                Flavor::of(base),
                Flavor::Claude | Flavor::Cursor | Flavor::T3
            )
            .then(|| (a.name.clone(), base.to_owned()))
        })
        .collect();
    if let Pick::Agent(name, command) = default_pick(feature, stage, workflow, installed)
        && !agents.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name))
    {
        agents.push((name, command));
    }
    let picked = feature.stage_agents.get(&stage).cloned();
    let lit = picked.or(match stage_pick(feature, stage, workflow, installed) {
        Pick::Agent(name, _) => Some(name),
        _ => None,
    });
    let allow_same = workflow.is_some_and(|w| w.allow_same_reviewer);
    let builder = (stage == Stage::AiReview && !allow_same)
        .then(|| builder_of(feature, workflow, installed))
        .flatten();
    let builder = builder.as_ref().map(|(n, c)| (n.as_str(), c.as_str()));
    agents
        .into_iter()
        .map(|(name, command)| AgentChip {
            icon: AgentIcon::from_agent(&name),
            lit: lit
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(&name)),
            off: same_agent(&name, &command, builder),
            name,
        })
        .collect()
}

/// The program of the CLI agent T3 Code names `agent` (`ThreadRow::agent`).
fn t3_agent_program(agent: &str) -> &str {
    match agent {
        "cursor" => "cursor-agent",
        other => other,
    }
}

/// Who built `feature`'s code, when T3 Code reviews it with `agent`
/// (`ThreadRow::agent`) and that is them: the page can't refuse that, as
/// the model is picked in T3 Code's draft.
pub(crate) fn t3_reviews_its_build(
    feature: &Feature,
    workflow: Option<&Workflow>,
    installed: &[CliAgent],
    agent: &str,
) -> Option<String> {
    if workflow.is_some_and(|w| w.allow_same_reviewer) {
        return None;
    }
    let (name, command) = builder_of(feature, workflow, installed)?;
    let same = same_agent(agent, t3_agent_program(agent), Some((&name, &command)));
    same.then_some(name)
}

/// The chip after the one lit among `chips` that can be picked, round the
/// ends; `None` when no other can.
fn next_chip(chips: &[AgentChip]) -> Option<&AgentChip> {
    let at = chips.iter().position(|c| c.lit);
    let start = at.map_or(0, |i| i + 1);
    (0..chips.len())
        .map(|k| &chips[(start + k) % chips.len()])
        .find(|c| !c.off && !c.lit)
}

/// `stage` as file names have it: `discover`, `ai-review`.
fn stage_key(stage: Stage) -> String {
    serde_json::to_value(stage)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| stage.label().to_lowercase())
}

/// The values the stage templates are rendered with. `names` holds the
/// projects' names (`Store::names`): the home one, and each link's.
pub(crate) fn vars(
    feature: &Feature,
    names: &PerProject<String>,
    data: &Path,
    stage: Stage,
) -> Vars {
    let path = |p: Option<PathBuf>| p.map(|p| p.display().to_string()).unwrap_or_default();
    let name_of = |project: Option<&str>| match project {
        None => names.home.clone(),
        Some(id) => names
            .links
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned()),
    };
    let linked = !feature.links.is_empty();
    let repos = if feature.many_repos() {
        feature
            .repos
            .iter()
            .map(|r| {
                let what = if r.touched {
                    "changed by this feature"
                } else {
                    "context only"
                };
                if linked {
                    format!(
                        "- {} (project {}): {} ({what})",
                        r.key(),
                        name_of(r.project.as_deref()),
                        r.worktree.display()
                    )
                } else {
                    format!("- {}: {} ({what})", r.name, r.worktree.display())
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        feature
            .repos
            .first()
            .map(|r| r.name.clone())
            .unwrap_or_default()
    };
    let projects = if linked {
        feature
            .folders()
            .into_iter()
            .map(|(project, dir)| {
                let home = if project.is_none() { ", home" } else { "" };
                format!("- {}{home}: {}", name_of(project), dir.display())
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    let graphs = (graph::folders(feature).iter())
        .map(|folder| format!("- {}", graph::file(&folder.dir).display()))
        .collect::<Vec<_>>()
        .join("\n");
    [
        ("feature.title", feature.title.clone()),
        ("feature.slug", feature.slug.clone()),
        ("feature.dir", feature.dir.display().to_string()),
        ("feature.data", data.display().to_string()),
        ("project.name", names.home.clone()),
        ("artifact.path", path(artifacts::path(data, stage))),
        ("brief.path", path(artifacts::path(data, Stage::Discover))),
        ("plan.path", path(artifacts::path(data, Stage::Plan))),
        (
            "evidence.path",
            data.join(verify::EVIDENCE).display().to_string(),
        ),
        ("evidence", String::new()),
        ("branch", feature.branch()),
        (
            "other_repos",
            layout::others_in_root(feature)
                .into_iter()
                .map(|(name, main)| format!("- {name}: {}", main.display()))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "multi_repo",
            if feature.many_repos() { "yes" } else { "" }.to_owned(),
        ),
        ("repos", repos),
        ("projects", projects),
        ("graphs", graphs),
        ("comments", String::new()),
        ("comments.path", String::new()),
        ("missing", String::new()),
        ("absent", String::new()),
        ("fixes", String::new()),
        ("fixes.path", String::new()),
        ("fixes.what", String::new()),
        ("review.diffs", String::new()),
        ("review.sent", String::new()),
        ("review.dismissed", String::new()),
        ("review.open", String::new()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// What the feature panel says of `run` under its stage: its agent's state,
/// as herdr has it (`status`) and the watch saw it (`watch`).
pub(crate) fn agent_line(
    run: &StageRun,
    stage: Stage,
    status: Option<AgentStatus>,
    watch: Option<&Watch>,
    now: u64,
) -> String {
    let file = artifacts::name(stage).unwrap_or("its file");
    if stage == Stage::Build {
        // Its `ready` says Build was marked done: never a review.
        if let Some(stop) = run.stop {
            return format!(
                "milestone {} of {} done \u{b7} Enter to continue",
                stop.done, stop.total
            );
        }
        if watch.is_some_and(|w| w.checking.is_some()) {
            return "checking the code".into();
        }
    } else if stage == Stage::AiReview {
        if run.ready.is_some() {
            return "findings read".into();
        }
        if watch.is_some_and(|w| w.checking.is_some()) {
            return "reading the findings".into();
        }
    } else if run.ready.is_some() {
        return format!("{file} ready for review");
    }
    if let Some((_, why)) = watch.and_then(|w| w.refused.as_ref()) {
        return format!("not ready: {why}");
    }
    if run.t3 == Some(T3Run::Draft) {
        return "draft in T3 Code \u{b7} pick a model and send it".into();
    }
    let started = format!("started {}", projects::ago(now, run.started));
    if !run.live() {
        return match run.resumes() {
            true => format!("{} closed \u{b7} can be resumed", run.agent),
            false => format!("{} closed \u{b7} {started}", run.agent),
        };
    }
    let missing = watch.map(|w| w.missing.as_slice()).unwrap_or_default();
    if run.nudged {
        return match missing.first() {
            Some(lack) => format!("nudged: {lack}"),
            None => format!("nudged to write {file}"),
        };
    }
    let doing = match status {
        Some(AgentStatus::Working) if stage == Stage::AiReview => "reviewing",
        Some(AgentStatus::Working) => "working",
        Some(AgentStatus::Blocked) => "needs you",
        Some(AgentStatus::Idle | AgentStatus::Done) => "waiting for you",
        _ => "running",
    };
    format!("{} {doing} \u{b7} {started}", run.agent)
}

/// The stamp of the file at `path`, if it is there.
fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let at = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Some((at, meta.len()))
}

/// How the watch finds `feature`'s `stage` file at `path`, written at or
/// after `after` to count. A build log with some milestones ticked but not
/// all is `Partial`.
fn read_file(feature: &Feature, stage: Stage, path: &Path, after: u64) -> File {
    let Some(stamp) = stamp(path) else {
        return File::Absent;
    };
    if stamp.0 < after {
        return File::Old;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match validate(feature, stage, &text) {
            Ok(()) => File::Good(stamp),
            Err(lacks) => match partial(stage, &text) {
                Some((done, total)) => File::Partial(stamp, done, total),
                None => File::Bad(stamp, lacks),
            },
        },
        Err(_) => File::Absent,
    }
}

/// The milestones ticked and in all of a `stage` file reading `text` that
/// isn't good, when it is a build log with some ticked but not all.
fn partial(stage: Stage, text: &str) -> Option<(u32, u32)> {
    if stage != Stage::Build {
        return None;
    }
    let (done, total) = artifacts::milestones(text);
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    (done > 0 && done < total).then(|| (count(done), count(total)))
}

/// What the stage is called in "<Stage> of “Title”" and approvals.
fn doc_word(stage: Stage) -> &'static str {
    match stage {
        Stage::Discover => "Brief",
        Stage::Plan => "Plan",
        _ => stage.label(),
    }
}

impl Gerdr {
    /// Note in feature `slug`'s history that `stage`'s agent couldn't
    /// start, `why`: the toast goes, the activity keeps it.
    fn log_not_started(&mut self, project: &str, slug: &str, stage: Stage, why: &str) {
        if let Some(f) = self.feature_mut(project, slug) {
            let what = format!("{} agent couldn't start: {why}", stage.label());
            f.log(feature::now(), what);
            self.save_feature(project, slug);
        }
    }

    /// The chips on `stage`'s page of feature `slug` (see `stage_chips`).
    pub(crate) fn chips_of(&self, project: &str, slug: &str, stage: Stage) -> Vec<AgentChip> {
        let workflow = self.projects.project(project).map(|p| &p.workflow);
        self.feature(project, slug).map_or_else(Vec::new, |f| {
            stage_chips(f, stage, workflow, &self.cli_agents)
        })
    }

    /// Run `stage` of feature `slug` with the agent `name`, from its next
    /// start on: one that runs now goes on. The one the project gives it
    /// anyway needs no pick, so the project's file stays what it follows.
    pub(crate) fn pick_stage_agent(&mut self, project: &str, slug: &str, stage: Stage, name: &str) {
        let chips = self.chips_of(project, slug, stage);
        if !chips.iter().any(|c| c.name == name && !c.off && !c.lit) {
            return;
        }
        let workflow = self.projects.project(project).map(|p| &p.workflow);
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let by_default = matches!(
            default_pick(feature, stage, workflow, &self.cli_agents),
            Pick::Agent(agent, _) if agent.eq_ignore_ascii_case(name)
        );
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if by_default {
            feature.stage_agents.remove(&stage);
        } else {
            feature.stage_agents.insert(stage, name.to_owned());
        }
        self.save_feature(project, slug);
    }

    /// Pick the next agent `stage`'s chips offer (Ctrl+Shift+A).
    pub(crate) fn cycle_stage_agent(&mut self, project: &str, slug: &str, stage: Stage) {
        let chips = self.chips_of(project, slug, stage);
        if let Some(next) = next_chip(&chips) {
            self.pick_stage_agent(project, slug, stage, &next.name);
        }
    }

    /// Start `stage`'s agent for feature `slug` (`fresh`: a new one even
    /// when its last run's pane is still there).
    pub fn start_stage_agent(&mut self, project: &str, slug: &str, stage: Stage, fresh: bool) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if needs(stage).is_none() {
            return self.show_warning(format!("No agent runs {} yet", stage.label()));
        }
        if let Err(why) = startable(feature, stage) {
            return self.show_warning(why);
        }
        if !fresh && feature.runs.get(&stage).is_some_and(|r| r.t3.is_some()) {
            return self.show_t3_stage(project, slug, stage);
        }
        let pane = feature.runs.get(&stage).and_then(|r| r.pane.clone());
        // The first agent gets a failed setup run again, like a fresh one.
        let first = !feature.runs.contains_key(&stage);
        if !fresh && let Some(pane) = pane.filter(|p| self.herdr_pane_place(p).is_some()) {
            return self.focus_agent(&agents::AgentKey::Herdr(pane));
        }
        if !fresh && feature.runs.get(&stage).is_some_and(StageRun::resumes) {
            return self.resume_stage_agent(project, slug, stage);
        }
        if stage == Stage::Build && self.setup_before_build(project, slug, fresh || first) {
            return;
        }
        self.launch_stage_agent(project, slug, stage, &[]);
    }

    /// Go back to the chat of `stage`'s last agent of feature `slug`, its
    /// tab closed, in a new tab. The stage stays where it was: a done one
    /// stays done, and an open one waits for the agent to work again.
    pub(crate) fn resume_stage_agent(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if feature.status != Status::Open {
            let title = &feature.title;
            return self.show_warning(format!("\u{201c}{title}\u{201d} isn't open"));
        }
        let Some(run) = feature.runs.get(&stage).filter(|r| r.resumes()) else {
            return self.show_warning(format!("{} has no chat to resume", stage.label()));
        };
        // Its tab back since the page was drawn only shows it: that changes
        // nothing. A chat to go on with would.
        let live = (run.pane.as_deref()).is_some_and(|p| self.herdr_pane_place(p).is_some());
        if !live && let Some(why) = self.stage_lock_warning(project, slug, stage) {
            return self.show_warning(why);
        }
        // Its tab came back since the page was drawn: one chat, one tab.
        if let Some(pane) = (run.pane.clone()).filter(|p| self.herdr_pane_place(p).is_some()) {
            return self.focus_agent(&agents::AgentKey::Herdr(pane));
        }
        let Some(session) = run.session() else {
            tracing::warn!("no chat id in {}", session_file(&run.prompt).display());
            let fresh = match feature.current() == Some(stage) {
                true => "; Ctrl+R starts a new agent",
                false => "",
            };
            return self.show_warning(format!(
                "Couldn't find {}'s chat on {}{fresh}",
                run.agent,
                stage.label()
            ));
        };
        let data = store::dir(None, project, slug);
        let command = resume_line(&run.base, &session, &feature.agent_dirs(&data));
        let agent = run.agent.clone();
        let opened = self.open_feature_tab(project, slug, &feature.dir, Some(command));
        let Some(pane) = opened else {
            tracing::warn!("no tab for {agent}'s chat on {}", stage.label());
            return self.show_error(format!("Couldn't open a tab for {agent}'s chat"));
        };
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(r) = f.runs.get_mut(&stage) else {
            return;
        };
        r.pane = Some(pane);
        f.log(
            feature::now(),
            format!("{} agent resumed: {agent}", stage.label()),
        );
        self.save_feature(project, slug);
        self.features
            .agents
            .remove(&(project.to_owned(), slug.to_owned(), stage));
        self.show_success(format!("Resumed {agent} on {}", stage.label()));
    }

    /// Start a new run of `stage`'s agent, its prompt rendered with `extra`
    /// over the feature's values (`comments` when it is sent back with
    /// them, `evidence` for a Build agent fixing Verify's failures).
    /// Whether it started.
    pub(crate) fn launch_stage_agent(
        &mut self,
        project: &str,
        slug: &str,
        stage: Stage,
        extra: &[(&str, String)],
    ) -> bool {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        let Some(need) = needs(stage) else {
            return false;
        };
        self.refresh_feature_graphs(project, slug, false);
        let names = self.projects.names(&feature);
        let workflow = self.projects.project(project).map(|p| &p.workflow);
        let picked = stage_pick(&feature, stage, workflow, &self.cli_agents);
        let (agent, base) = match picked {
            Pick::Agent(agent, base) => (agent, base),
            Pick::NoCli => {
                let why = "No coding agent CLI is installed: install Claude Code, Codex or Cursor Agent, or set one under [agents] in Settings";
                crate::problems::report("features", why);
                self.show_error(why.to_owned());
                self.log_not_started(project, slug, stage, "no coding agent CLI installed");
                return false;
            }
            Pick::Refused(why) => {
                self.log_not_started(project, slug, stage, &why);
                self.show_warning(why);
                return false;
            }
        };
        let in_t3 = Flavor::of(&base) == Flavor::T3;
        if in_t3 && let Err(why) = self.t3_can_draft() {
            self.log_not_started(project, slug, stage, &why);
            return false;
        }
        let data = store::dir(None, project, slug);
        let last = feature.runs.get(&stage);
        let n = feature.next_run(stage);
        let prompt = data
            .join("prompts")
            .join(format!("{}-{n}.md", stage_key(stage)));
        let mut values = vars(&feature, &names, &data, stage);
        if stage == Stage::AiReview {
            values.extend(crate::feature_review::review_vars(&feature));
        }
        for (key, value) in extra {
            values.insert((*key).to_owned(), value.clone());
        }
        let version = prompts::builtin(need.template).map_or(0, |(_, v)| v);
        let text = match prompts::render_builtin(need.template, &values) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!("rendering the {} prompt: {err}", need.template);
                self.show_error(format!(
                    "Couldn't write the {} prompt: {err}",
                    stage.label()
                ));
                self.log_not_started(project, slug, stage, "its prompt couldn't be written");
                return false;
            }
        };
        let written = std::fs::create_dir_all(data.join("prompts"))
            .and_then(|()| std::fs::write(&prompt, text));
        if let Err(err) = written {
            tracing::warn!("writing {}: {err}", prompt.display());
            self.show_error(format!("Couldn't write {}: {err}", prompt.display()));
            self.log_not_started(project, slug, stage, "its prompt couldn't be written");
            return false;
        }
        let command = if in_t3 {
            base.clone()
        } else {
            command_line(&base, &prompt, &feature.agent_dirs(&data))
        };
        // The agent runs in a tab of the feature's own workspace; T3 Code's
        // once the user sends its draft.
        let pane = match in_t3 {
            true => None,
            false => self.open_feature_tab(project, slug, &feature.dir, Some(command.clone())),
        };
        let now = feature::now();
        let run = StageRun {
            n,
            agent: agent.clone(),
            command,
            base,
            pane,
            t3: in_t3.then_some(T3Run::Draft),
            prompt,
            template: format!("builtin:{}@{version}", need.template),
            started: now,
            nudged: false,
            ready: None,
            sent_back: last.map_or(0, |r| r.sent_back),
            stop: None,
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return false;
        };
        feature.runs.insert(stage, run);
        feature.log(
            now,
            format!("{} agent started: {agent} (run {n})", stage.label()),
        );
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.features
            .agents
            .remove(&(project.to_owned(), slug.to_owned(), stage));
        if in_t3 {
            self.t3_stage_drafted(project, slug, stage, &title);
            return true;
        }
        self.show_success(format!(
            "Started {agent} on {} of \u{201c}{title}\u{201d}",
            stage.label()
        ));
        true
    }

    /// Look at every running stage agent: done with a good file makes its
    /// gate wait (a Build agent's code is read, see `feature_build`); a
    /// Build agent done after a milestone waits at the milestone gate;
    /// done without a good file is nudged once. Called after each read of
    /// herdr's agents.
    pub fn stage_agents_tick(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        self.fixing_tick(&project);
        let Some(all) = self.features.all.get(&project) else {
            return;
        };
        let runs: Vec<(Feature, Stage, StageRun)> = all
            .iter()
            .filter(|f| f.status == Status::Open)
            .filter_map(|f| {
                let stage = f.current()?;
                needs(stage)?;
                let run = f.runs.get(&stage)?;
                // A Build run marked done before is back: its code changed.
                (run.ready.is_none() || stage == Stage::Build)
                    .then(|| (f.clone(), stage, run.clone()))
            })
            .collect();
        let now = Instant::now();
        for (feature, stage, run) in runs {
            let key = (project.clone(), feature.slug.clone(), stage);
            if self.t3_stage_thread_sent(&feature, stage, &run) {
                continue;
            }
            let (status, alive) = self.stage_run_state(&run);
            let data = store::dir(None, &project, &feature.slug);
            let Some(path) = artifacts::path(&data, stage) else {
                continue;
            };
            let build = stage == Stage::Build;
            let back = build && run.ready.is_some();
            let working = matches!(status, Some(AgentStatus::Working | AgentStatus::Blocked));
            if back || (build && working && run.stop.is_some()) {
                // Back at Build, or going on past its stop: nothing waits.
                if let Some(r) = self
                    .feature_mut(&project, &feature.slug)
                    .and_then(|f| f.runs.get_mut(&stage))
                {
                    r.ready = None;
                    r.stop = None;
                    self.save_feature(&project, &feature.slug);
                }
            }
            let watch = self.features.agents.entry(key.clone()).or_default();
            if watch.n != run.n {
                // Seen first now: after a restart, a file there already
                // stands for the work done. A build log of any age counts:
                // a fresh Build agent goes on from it.
                *watch = Watch {
                    n: run.n,
                    after: if build { 0 } else { run.started },
                    // A thread of T3 Code's is there once it was sent.
                    worked: run.t3_thread().is_some()
                        || stamp(&path).is_some_and(|s| s.0 >= run.started),
                    ..Watch::default()
                };
            }
            if back {
                // Its code changed after it was done: read it again once
                // the agent rests, whatever it did since.
                watch.worked = true;
            }
            if build && working {
                // What it was told it lacked is its to mend: read anew.
                watch.refused = None;
            }
            let file = read_file(&feature, stage, &path, watch.after);
            let verdict = judge(
                watch,
                status,
                alive,
                &file,
                stage == Stage::Discover,
                run.nudged,
                now,
            );
            match verdict {
                Verdict::Wait => {}
                Verdict::Ready(stamp)
                    if matches!(stage, Stage::Plan | Stage::Build | Stage::AiReview) =>
                {
                    // A refused file is read again now and then: the
                    // user may have undone the agent's changes, or
                    // committed them.
                    let refused = watch.refused.as_ref().is_some_and(|(s, _)| *s == stamp)
                        && watch
                            .refused_at
                            .is_some_and(|at| now.duration_since(at) < RECHECK);
                    if watch.checking.is_none() && !refused {
                        watch.checking = Some(stamp);
                        match stage {
                            Stage::Build => {
                                self.check_build_code(&project, &feature.slug, run.n, stamp);
                            }
                            Stage::Plan => {
                                self.check_plan_code(&project, &feature.slug, run.n, stamp);
                            }
                            _ => self.check_review_code(&project, &feature.slug, run.n, stamp),
                        }
                    }
                }
                Verdict::Ready(_) => self.stage_ready(&project, &feature.slug, stage),
                Verdict::Stop { done, total } => {
                    self.milestone_stop(&project, &feature.slug, run.n, done, total);
                    if !alive && run.live() {
                        self.forget_pane(&project, &feature.slug, stage);
                    }
                }
                Verdict::Nudge { absent } => {
                    let missing = watch.missing.join("; ");
                    self.nudge_stage_agent(&feature, stage, &run, absent, missing);
                }
                Verdict::Gone if run.live() => {
                    self.forget_pane(&project, &feature.slug, stage);
                }
                Verdict::Gone => {}
            }
        }
    }

    /// What agent `key` does, if known, and whether it is still there.
    pub(crate) fn agent_key_state(&self, key: &agents::AgentKey) -> (Option<AgentStatus>, bool) {
        match key {
            agents::AgentKey::T3(id) => self.t3_thread_state(id),
            agents::AgentKey::Herdr(pane) => {
                let status = (self.agents.iter())
                    .find(|a| a.pane_id == *pane)
                    .map(|a| a.agent_status);
                (
                    status,
                    status.is_some() || self.herdr_pane_place(pane).is_some(),
                )
            }
            _ => (None, false),
        }
    }

    /// What `run`'s agent does, if known, and whether it is still there. A
    /// draft in T3 Code is, as far as gerdr can tell: it can't see drafts.
    pub(crate) fn stage_run_state(&self, run: &StageRun) -> (Option<AgentStatus>, bool) {
        match run.agent_key() {
            Some(key) => self.agent_key_state(&key),
            None => (None, run.t3.is_some()),
        }
    }

    /// The tab of `stage`'s agent of feature `slug` is gone: its run
    /// forgets it.
    fn forget_pane(&mut self, project: &str, slug: &str, stage: Stage) {
        if let Some(f) = self.feature_mut(project, slug)
            && let Some(r) = f.runs.get_mut(&stage)
        {
            r.pane = None;
            let what = match r.t3.take() {
                Some(_) => "agent's thread is gone from T3 Code",
                None => "agent's tab closed",
            };
            f.log(feature::now(), format!("{} {what}", stage.label()));
            self.save_feature(project, slug);
        }
    }

    /// The Build agent of feature `slug` (run `n`) stopped after milestone
    /// `done` of `total`: the milestone gate waits, and the user is told.
    /// The same stop again is nothing new.
    fn milestone_stop(&mut self, project: &str, slug: &str, n: u32, done: u32, total: u32) {
        // A fresh agent waits on the setup to take over from this one: its
        // stop is over (see `setup_before_build`).
        let key = (project.to_owned(), slug.to_owned());
        if self.features.build.pending.contains(&key) {
            return;
        }
        let at = feature::now();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if feature.current() != Some(Stage::Build) {
            return;
        }
        let Some(run) = feature.runs.get_mut(&Stage::Build).filter(|r| r.n == n) else {
            return;
        };
        if !stop_is_new(run.stop, done, total) {
            return;
        }
        run.stop = Some(Stop { done, total, at });
        let pane = run.pane.clone();
        feature.log(
            at,
            format!("Build stopped after milestone {done} of {total}"),
        );
        let title = feature.title.clone();
        self.save_feature(project, slug);
        let summary = format!("Build of \u{201c}{title}\u{201d}: milestone {done} of {total} done");
        self.notify_feature(project, slug, pane, &summary);
        self.show_success(summary);
    }

    /// Tell `run`'s agent once that its file isn't there (`absent`) or
    /// lacks `missing`.
    fn nudge_stage_agent(
        &mut self,
        feature: &Feature,
        stage: Stage,
        run: &StageRun,
        absent: bool,
        missing: String,
    ) {
        let Some(agent) = run.agent_key() else {
            return;
        };
        let project = feature.project.clone();
        let slug = feature.slug.clone();
        let names = self.projects.names(feature);
        let data = store::dir(None, &project, &slug);
        let mut values = vars(feature, &names, &data, stage);
        values.insert(
            "absent".into(),
            if absent { "yes".into() } else { String::new() },
        );
        values.insert("missing".into(), missing.clone());
        let text = match prompts::render_builtin("nudge", &values) {
            Ok(text) => text.trim().to_owned(),
            Err(err) => return tracing::warn!("rendering the nudge: {err}"),
        };
        let sent = match &agent {
            agents::AgentKey::T3(id) => self.t3_send(id, &text),
            agents::AgentKey::Herdr(pane) => match self.herdr.as_mut() {
                Some(herdr) => herdr.send_line(pane, &text),
                None => Err("herdr isn't connected".into()),
            },
            _ => return,
        };
        let now = feature::now();
        let Some(f) = self.feature_mut(&project, &slug) else {
            return;
        };
        let Some(r) = f.runs.get_mut(&stage) else {
            return;
        };
        // Once, sent or not: a failure isn't tried again every round.
        r.nudged = true;
        match sent {
            Ok(()) => {
                let why = if absent {
                    "no file".to_owned()
                } else {
                    missing
                };
                f.log(now, format!("{} agent nudged: {why}", stage.label()));
            }
            Err(err) => {
                tracing::warn!(
                    "nudging the {} agent of {project}/{slug}: {err}",
                    stage.label()
                );
                f.log(
                    now,
                    format!("{} agent couldn't be nudged: {err}", stage.label()),
                );
            }
        }
        self.save_feature(&project, &slug);
    }

    /// `stage` of feature `slug` is done with a good file: its gate waits.
    fn stage_ready(&mut self, project: &str, slug: &str, stage: Stage) {
        let now = feature::now();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if feature.current() != Some(stage) {
            return;
        }
        let Some(run) = feature.runs.get_mut(&stage) else {
            return;
        };
        if run.ready.is_some() {
            return;
        }
        run.ready = Some(now);
        let pane = run.pane.clone();
        feature.log(now, format!("{} ready for review", doc_word(stage)));
        // As written, to tell the agent at send back whether the user
        // edited it in the review.
        if let Err(err) = artifacts::keep_written(&store::dir(None, project, slug), stage) {
            tracing::warn!("keeping {project}/{slug}'s file as written: {err}");
        }
        let title = feature.title.clone();
        self.save_feature(project, slug);
        let summary = format!(
            "{} of \u{201c}{title}\u{201d} is ready for review",
            doc_word(stage)
        );
        self.notify_feature(project, slug, pane, &summary);
        self.show_success(summary);
    }

    /// Notify `summary` of feature `slug`, unless its agent's tab (`pane`)
    /// is in sight.
    pub(crate) fn notify_feature(
        &mut self,
        project: &str,
        slug: &str,
        pane: Option<String>,
        summary: &str,
    ) {
        let in_sight = self.host_focused
            && pane
                .as_deref()
                .and_then(|p| self.herdr_pane_place(p))
                .is_some_and(|(w, t)| self.is_visible(w, t));
        if !in_sight && let Some(notifier) = self.notifier.as_mut().filter(|_| self.notify) {
            let key = pane.map_or_else(
                || format!("feature:{project}/{slug}"),
                |p| agents::AgentKey::Herdr(p).to_string(),
            );
            let body = self
                .projects
                .project(project)
                .map_or_else(String::new, |p| p.name.clone());
            notifier.send(&key, summary, &body);
        }
    }

    /// The code of feature `slug` was read for run `n` of its Plan agent,
    /// whose file had `stamp`: unchanged makes the plan ready; else the
    /// agent broke read-only, and the gate stays shut.
    pub(crate) fn plan_code_checked(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: Stamp,
        result: Result<Reading, String>,
    ) {
        let key = (project.to_owned(), slug.to_owned(), Stage::Plan);
        let Some(watch) = self.features.agents.get_mut(&key).filter(|w| w.n == n) else {
            return;
        };
        watch.checking = None;
        watch.refused_at = Some(Instant::now());
        let reading = match result {
            Ok(reading) => reading,
            Err(why) => {
                // Read again now and then; said once until it changes.
                let line = format!("couldn't read the code: {why}");
                let again = watch.refused.as_ref() == Some(&(stamp, line.clone()));
                watch.refused = Some((stamp, line));
                if again {
                    return;
                }
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't check the Plan agent's code: {why}"));
            }
        };
        let mut changed: Vec<String> = reading.dirty.clone();
        changed.extend(reading.stray.iter().cloned());
        changed.extend(
            reading
                .commits
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|(repo, _)| repo.clone()),
        );
        changed.sort();
        changed.dedup();
        if changed.is_empty() {
            watch.refused = None;
            return self.stage_ready(project, slug, Stage::Plan);
        }
        let repos = changed.join(", ");
        let why = format!("the agent changed the code in {repos}");
        let again = watch.refused.as_ref() == Some(&(stamp, why.clone()));
        watch.refused = Some((stamp, why));
        if again {
            return;
        }
        let text = format!(
            "The Plan agent changed the code in {repos}; undo its changes for the plan to go up for review, or start a fresh agent"
        );
        tracing::warn!("feature {project}/{slug}: {text}");
        if let Some(feature) = self.feature_mut(project, slug) {
            feature.log(
                feature::now(),
                format!("Plan refused: the agent changed the code in {repos}"),
            );
            self.save_feature(project, slug);
        }
        self.show_warning(text);
    }

    /// The gates waiting for the user, oldest first, over every project.
    pub fn waiting_gates(&self) -> Vec<Gate> {
        let manual = |f: &Feature| {
            let n = self.manual_counts(f)?;
            let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
            Some((count(n.passed), count(n.total())))
        };
        gates(&self.features.all, manual)
    }

    /// Approve `stage` of feature `slug`: its file as it is now. Marks
    /// the stage done. False when it isn't approved, the user told why.
    ///
    /// Build isn't approved: with its `build.md` good, its code is read now
    /// as if the agent were done, and gerdr marks it done if it holds (see
    /// `build_code_checked`); false.
    pub fn approve_stage(&mut self, project: &str, slug: &str, stage: Stage) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            return false;
        };
        let data = store::dir(None, project, slug);
        let path = artifacts::path(&data, stage);
        let text = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        if let Err(why) = approval(feature, stage, text.as_deref()) {
            self.show_warning(why);
            return false;
        }
        if stage == Stage::Build {
            let n = feature.runs.get(&stage).map_or(0, |r| r.n);
            let title = feature.title.clone();
            let stamp = path.as_deref().and_then(stamp).unwrap_or_default();
            let key = (project.to_owned(), slug.to_owned(), stage);
            let watch = self.features.agents.entry(key).or_default();
            if watch.n != n {
                *watch = Watch::of(n);
            }
            if watch.checking.is_some() {
                self.show_warning(format!(
                    "Checking the code of \u{201c}{title}\u{201d} already"
                ));
                return false;
            }
            // Asked for: what it finds is said again.
            watch.refused = None;
            watch.checking = Some(stamp);
            self.check_build_code(project, slug, n, stamp);
            return false;
        }
        // A copy that can't be kept only costs saying it changed later.
        if let Err(why) = artifacts::keep_approved(&data, stage) {
            tracing::warn!("keeping {project}/{slug}'s file as approved: {why}");
            let text = format!("Approved, but later edits to this file won't show: {why}");
            self.show_warning(text);
        }
        let now = feature::now();
        let Some(feature) = self.feature_mut(project, slug) else {
            return false;
        };
        feature.tick(now, "you", None);
        let done = format!("{} approved", doc_word(stage));
        feature.log(now, done.clone());
        self.save_feature(project, slug);
        self.features
            .agents
            .remove(&(project.to_owned(), slug.to_owned(), stage));
        self.feature_cursor_to_current_of(project, slug);
        self.show_success(done);
        true
    }

    /// Send `stage` of feature `slug` back to its agent with `comments`
    /// (Markdown, as `review::markdown` writes it): pasted into its pane,
    /// else a fresh agent is started with them.
    ///
    /// The comments go to a file next to the prompts, and the pane is told
    /// where in one line: a terminal agent would send a pasted line break
    /// as Enter.
    pub fn send_back_stage(&mut self, project: &str, slug: &str, stage: Stage, comments: String) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if feature.status != Status::Open || feature.current() != Some(stage) {
            return self.show_warning(format!("{} isn't waiting for review", stage.label()));
        }
        let Some(run) = feature.runs.get(&stage).cloned() else {
            self.launch_stage_agent(project, slug, stage, &[("comments", comments)]);
            return;
        };
        let data = store::dir(None, project, slug);
        let comments = with_edits_told(comments, &data, stage);
        let back = run.sent_back + 1;
        let file =
            data.join("prompts")
                .join(format!("{}-{}-back-{back}.md", stage_key(stage), run.n));
        let alive = run
            .pane
            .as_deref()
            .is_some_and(|p| self.herdr_pane_place(p).is_some());
        let mut sent = Err(String::from("its tab is gone"));
        if alive && let Some(pane) = run.pane.clone() {
            sent = std::fs::create_dir_all(data.join("prompts"))
                .and_then(|()| std::fs::write(&file, &comments))
                .map_err(|e| format!("writing {}: {e}", file.display()))
                .and_then(|()| {
                    let names = self.projects.names(&feature);
                    let mut values = vars(&feature, &names, &data, stage);
                    values.insert("comments".into(), comments.clone());
                    values.insert("comments.path".into(), file.display().to_string());
                    prompts::render_builtin("send-back", &values).map_err(|e| e.to_string())
                })
                .and_then(|line| self.tell_agent_pane(&pane, line.trim()));
        }
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(r) = f.runs.get_mut(&stage) else {
            return;
        };
        r.sent_back = back;
        r.ready = None;
        r.nudged = false;
        let agent = r.agent.clone();
        let what = doc_word(stage).to_lowercase();
        f.log(now, format!("{} sent back ({back})", doc_word(stage)));
        self.save_feature(project, slug);
        let key = (project.to_owned(), slug.to_owned(), stage);
        match sent {
            Ok(()) => {
                let watch = self.features.agents.entry(key).or_default();
                *watch = Watch {
                    n: run.n,
                    after: now,
                    ..Watch::default()
                };
                self.show_success(format!("Sent the {what} back to {agent}"));
            }
            Err(why) => {
                if alive {
                    tracing::warn!("sending {project}/{slug} back to its agent: {why}");
                }
                self.features.agents.remove(&key);
                self.launch_stage_agent(project, slug, stage, &[("comments", comments)]);
            }
        }
    }
}

/// `comments` sending `stage`'s file back, told too when the user edited
/// the file in the review since its agent wrote it, so it keeps those
/// edits.
fn with_edits_told(comments: String, data: &Path, stage: Stage) -> String {
    if !artifacts::edited_since_written(data, stage) {
        return comments;
    }
    let name = artifacts::name(stage).unwrap_or("the file");
    format!(
        "{}\n\nThe user also edited {name} by hand in the review since you wrote it. \
         Read it as it is now and keep those edits.\n",
        comments.trim_end()
    )
}

/// Whether a Build agent's stop after milestone `done` of `total` is new:
/// none yet, or another one.
fn stop_is_new(stop: Option<Stop>, done: u32, total: u32) -> bool {
    stop.is_none_or(|s| (s.done, s.total) != (done, total))
}

/// Whether `stage`'s agent can start for `feature`; else why not.
fn startable(feature: &Feature, stage: Stage) -> Result<(), String> {
    if feature.status != Status::Open {
        return Err(format!("\u{201c}{}\u{201d} isn't open", feature.title));
    }
    if feature.is_done(stage) {
        return Err(format!("{} is done; reopen it first", stage.label()));
    }
    match feature.current() {
        Some(current) if current == stage => Ok(()),
        Some(current) => Err(format!("Mark {} done first", current.label())),
        None => Err("Nothing left to do".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_sent_back_after_the_user_edited_it_says_so() {
        let dir = std::env::temp_dir().join(format!("gerdr-edits-told-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let brief = dir.join("brief.md");
        std::fs::write(&brief, "# Brief\n").unwrap();
        artifacts::keep_written(&dir, Stage::Discover).unwrap();
        let told = |dir: &Path| with_edits_told("Comments.\n".into(), dir, Stage::Discover);
        assert_eq!(told(&dir), "Comments.\n");
        std::fs::write(&brief, "# Brief, edited\n").unwrap();
        let text = told(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            text.starts_with("Comments.\n\nThe user also edited brief.md"),
            "{text}"
        );
    }
    use crate::{
        agent_icons::AgentIcon,
        cli_agents::Start,
        features::tests::{feature, repo},
    };

    const BRIEF: &str = "## Outcome\nx\n## In scope\n- a\n## Out of scope\n- b\n## Acceptance criteria\n- Given a, when b, then c\n";
    const PLAN: &str =
        "## Milestones\n- [ ] one\n## Repositories\n- api\n## Merge order\napi\nMigrations: none\n";

    fn run(ready: Option<u64>) -> StageRun {
        StageRun {
            n: 1,
            agent: "Claude Code".into(),
            command: "claude".into(),
            pane: Some("p1".into()),
            prompt: "/d/prompts/discover-1.md".into(),
            template: "builtin:discover@1".into(),
            started: 1_000,
            ready,
            ..StageRun::default()
        }
    }

    #[test]
    fn waiting_gates_are_oldest_first_over_projects() {
        let mut a = feature("a", vec![repo("api", true)], 0);
        a.runs.insert(Stage::Discover, run(Some(300)));
        let mut b = feature("b", vec![repo("api", true)], 1);
        b.runs.insert(Stage::Plan, run(Some(100)));
        // Not ready, or not its current stage: no gate.
        let mut c = feature("c", vec![repo("api", true)], 0);
        c.runs.insert(Stage::Discover, run(None));
        let mut d = feature("d", vec![repo("api", true)], 1);
        d.runs.insert(Stage::Discover, run(Some(50)));
        let mut e = feature("e", vec![repo("api", true)], 0);
        e.runs.insert(Stage::Discover, run(Some(200)));
        let all = BTreeMap::from([
            ("one".to_owned(), vec![a, c, d]),
            ("two".to_owned(), vec![b, e]),
        ]);
        let gates = gates(&all, |_| None);
        let got: Vec<(&str, &str, Stage, u64)> = gates
            .iter()
            .map(|g| (g.project.as_str(), g.slug.as_str(), g.stage, g.since))
            .collect();
        assert_eq!(
            got,
            vec![
                ("two", "b", Stage::Plan, 100),
                ("two", "e", Stage::Discover, 200),
                ("one", "a", Stage::Discover, 300),
            ]
        );
        assert_eq!(gates[0].title, "Feature b");
    }

    #[test]
    fn approval_refuses_and_says_why() {
        let f = feature("a", vec![repo("api", true)], 0);
        assert_eq!(approval(&f, Stage::Discover, Some(BRIEF)), Ok(()));
        assert_eq!(
            approval(&f, Stage::Discover, None),
            Err("There's no brief.md yet".into())
        );
        assert_eq!(
            approval(&f, Stage::Discover, Some("## Outcome\nx\n")),
            Err("brief.md isn't ready: no In scope section; no Out of scope section; no Acceptance criteria section".into())
        );
        assert_eq!(
            approval(&f, Stage::Plan, Some(PLAN)),
            Err("Mark Discover done first".into())
        );
        let planned = feature("a", vec![repo("api", true)], 1);
        assert_eq!(
            approval(&planned, Stage::Discover, Some(BRIEF)),
            Err("Discover is approved already".into())
        );
        assert_eq!(approval(&planned, Stage::Plan, Some(PLAN)), Ok(()));
        let mut gone = planned.clone();
        gone.status = Status::Abandoned;
        assert!(approval(&gone, Stage::Plan, Some(PLAN)).is_err());
        // AI review and Code review pass by their own checks only.
        let reviewing = feature("a", vec![repo("api", true)], 4);
        assert_eq!(reviewing.current(), Some(Stage::AiReview));
        let review = "{\"findings\": []}";
        assert_eq!(
            approval(&reviewing, Stage::AiReview, Some(review)),
            Err("AI review is done once its findings are decided".into())
        );
        let coding = feature("a", vec![repo("api", true)], 5);
        assert_eq!(coding.current(), Some(Stage::CodeReview));
        assert_eq!(
            approval(&coding, Stage::CodeReview, None),
            Err("Approve Code review from the feature's diff".into())
        );
    }

    #[test]
    fn a_multi_repo_plan_names_its_touched_repos() {
        let f = feature(
            "a",
            vec![repo("api", true), repo("web", true), repo("docs", false)],
            1,
        );
        assert_eq!(
            validate(&f, Stage::Plan, PLAN),
            Err(vec!["Repositories doesn't name web".into()])
        );
        let both = PLAN.replace("- api\n", "- api\n- web\n");
        assert_eq!(validate(&f, Stage::Plan, &both), Ok(()));
    }

    fn agent(name: &str, start: Start) -> CliAgent {
        CliAgent {
            name: name.into(),
            start,
            icon: AgentIcon::Claude,
        }
    }

    #[test]
    fn the_agent_is_the_configured_one_else_the_first_cli() {
        let installed = [
            agent("T3 Code", Start::T3),
            agent("Codex", Start::Command("codex".into())),
            agent("Claude Code", Start::Command("claude".into())),
        ];
        assert_eq!(
            pick_agent(None, &installed),
            Some(("Codex".into(), "codex".into()))
        );
        assert_eq!(
            pick_agent(Some("claude code"), &installed),
            Some(("Claude Code".into(), "claude".into()))
        );
        assert_eq!(
            pick_agent(Some("/opt/bin/aider --yes"), &installed),
            Some(("aider".into(), "/opt/bin/aider --yes".into()))
        );
        assert_eq!(pick_agent(Some(" "), &installed[..1]), None);
    }

    #[test]
    fn the_reviewer_is_not_the_builder() {
        const CODEX: (&str, &str) = ("Codex", "codex --add-dir /d -- \"$(cat /d/p.md)\"");
        let installed = [
            agent("T3 Code", Start::T3),
            agent("Codex", Start::Command("codex".into())),
            agent("Claude Code", Start::Command("claude".into())),
        ];
        let codex = Pick::Agent("Codex".into(), "codex".into());
        let claude = Pick::Agent("Claude Code".into(), "claude".into());
        assert_eq!(pick_reviewer(None, None, &installed, false), codex);
        assert_eq!(pick_reviewer(None, Some(CODEX), &installed, false), claude);
        assert_eq!(
            pick_reviewer(Some("claude code"), Some(CODEX), &installed, false),
            claude
        );
        // Set to the builder: refused, unless allowed.
        let Pick::Refused(why) = pick_reviewer(Some("Codex"), Some(CODEX), &installed, false)
        else {
            panic!("not refused");
        };
        assert!(why.starts_with("AI review is set to Codex, which built the code"));
        assert!(why.contains("allow_same_reviewer"));
        assert_eq!(
            pick_reviewer(Some("Codex"), Some(CODEX), &installed, true),
            codex
        );
        // No other agent: refused, or the builder when allowed.
        let only = &installed[..2];
        let Pick::Refused(why) = pick_reviewer(None, Some(CODEX), only, false) else {
            panic!("not refused");
        };
        assert!(why.starts_with("No agent but Codex, which built the code, is installed"));
        assert!(why.contains("allow_same_reviewer"));
        assert_eq!(pick_reviewer(None, Some(CODEX), only, true), codex);
        assert_eq!(
            pick_reviewer(None, None, &installed[..1], false),
            Pick::NoCli
        );
        // Built by Claude Code set as a command: the same agent.
        let by_claude = Some((
            "claude",
            "/usr/bin/claude --add-dir /d -- \"$(cat /d/p.md)\"",
        ));
        assert_eq!(pick_reviewer(None, by_claude, &installed, false), codex);
        let Pick::Refused(why) = pick_reviewer(Some("Claude Code"), by_claude, &installed, false)
        else {
            panic!("not refused");
        };
        assert!(why.starts_with("AI review is set to Claude Code, which built the code"));
        // Set as a command, the same program as the builder's.
        assert!(matches!(
            pick_reviewer(Some("codex --yes"), Some(CODEX), &installed, false),
            Pick::Refused(_)
        ));
    }

    #[test]
    fn findings_and_code_review_wait_at_their_gates() {
        use crate::feature::review_import::{AiReview, Finding};
        let finding = |state| Finding {
            state,
            ..Finding::default()
        };
        let mut f = feature("a", vec![repo("api", true)], 4);
        f.runs.insert(Stage::AiReview, run(None));
        f.review = Some(AiReview {
            findings: vec![
                finding(FindingState::Open),
                finding(FindingState::Open),
                finding(FindingState::Fix),
                finding(FindingState::Dismissed),
            ],
            ..AiReview::default()
        });
        // Read in only once the run is ready.
        assert_eq!(gate_of(&f, |_| None), None);
        f.runs.insert(Stage::AiReview, run(Some(500)));
        assert_eq!(
            gate_of(&f, |_| None),
            Some((Stage::AiReview, 500, GateKind::Findings { open: 2, fix: 1 }))
        );
        let review = f.review.as_mut().unwrap();
        review.findings[0].state = FindingState::Dismissed;
        review.findings[1].state = FindingState::Sent;
        assert_eq!(
            gate_of(&f, |_| None),
            Some((Stage::AiReview, 500, GateKind::Findings { open: 0, fix: 1 }))
        );
        f.review.as_mut().unwrap().findings[2].state = FindingState::Sent;
        assert_eq!(gate_of(&f, |_| None), None);
        let mut code = feature("b", vec![repo("api", true)], 5);
        assert_eq!(
            gate_of(&code, |_| None),
            Some((Stage::CodeReview, 2_000, GateKind::CodeReview))
        );
        code.marks.get_mut(&Stage::AiReview).unwrap().at = 3_000;
        assert_eq!(gate_of(&code, |_| None).map(|g| g.1), Some(3_000));
        // Manual test waits only while the brief has criteria.
        let testing = feature("c", vec![repo("api", true)], 6);
        assert_eq!(gate_of(&testing, |_| None), None);
        assert_eq!(gate_of(&testing, |_| Some((0, 0))), None);
        assert_eq!(
            gate_of(&testing, |_| Some((1, 2))),
            Some((
                Stage::ManualTest,
                2_000,
                GateKind::ManualTest {
                    passed: 1,
                    total: 2
                }
            ))
        );
        assert_eq!(
            approval(&testing, Stage::ManualTest, None),
            Err("Mark Manual test passed from its sheet".into())
        );
    }

    #[test]
    fn the_reviewer_line_says_it_reviews_then_reads() {
        let now = 1_000 + 120;
        let mut r = run(None);
        r.agent = "Codex".into();
        assert_eq!(
            agent_line(&r, Stage::AiReview, Some(AgentStatus::Working), None, now),
            "Codex reviewing \u{b7} started 2m ago"
        );
        let checking = Watch {
            checking: Some((1, 1)),
            ..Watch::default()
        };
        assert_eq!(
            agent_line(&r, Stage::AiReview, None, Some(&checking), now),
            "reading the findings"
        );
        r.ready = Some(1_100);
        assert_eq!(
            agent_line(&r, Stage::AiReview, None, None, now),
            "findings read"
        );
    }

    #[test]
    fn the_review_prompt_gets_its_file_and_the_diffs() {
        let f = feature("a", vec![repo("api", true)], 4);
        let mut v = vars(
            &f,
            &PerProject::all("Acme".to_owned()),
            Path::new("/data/a"),
            Stage::AiReview,
        );
        assert_eq!(v["artifact.path"], "/data/a/review.json");
        v.extend(crate::feature_review::review_vars(&f));
        let text = prompts::render_builtin("ai-review", &v).unwrap();
        assert!(text.contains("`git diff abc123...HEAD`"));
        assert!(text.contains("/data/a/brief.md") && text.contains("/data/a/plan.md"));
    }

    fn watch() -> Watch {
        Watch {
            n: 1,
            ..Watch::default()
        }
    }

    #[test]
    fn done_after_working_with_a_good_file_is_ready() {
        let t = Instant::now();
        let good = File::Good((5, 9));
        let mut w = watch();
        // Idle before it worked: still starting.
        assert_eq!(
            judge(
                &mut w,
                Some(AgentStatus::Idle),
                true,
                &good,
                false,
                false,
                t
            ),
            Verdict::Wait
        );
        assert_eq!(
            judge(
                &mut w,
                Some(AgentStatus::Working),
                true,
                &good,
                false,
                false,
                t
            ),
            Verdict::Wait
        );
        assert_eq!(
            judge(
                &mut w,
                Some(AgentStatus::Done),
                true,
                &good,
                false,
                false,
                t
            ),
            Verdict::Ready((5, 9))
        );
    }

    #[test]
    fn done_without_a_good_file_is_nudged_once() {
        let t = Instant::now();
        let mut w = watch();
        w.worked = true;
        let idle = Some(AgentStatus::Idle);
        assert_eq!(
            judge(&mut w, idle, true, &File::Absent, false, false, t),
            Verdict::Nudge { absent: true }
        );
        assert_eq!(
            judge(&mut w, idle, true, &File::Absent, false, true, t),
            Verdict::Wait
        );
        let bad = File::Bad((1, 1), vec!["no Outcome section".into()]);
        assert_eq!(
            judge(&mut w, idle, true, &bad, true, false, t),
            Verdict::Nudge { absent: false }
        );
        assert_eq!(w.missing, vec!["no Outcome section".to_owned()]);
        // A Discover chat without its brief yet is left to talk.
        assert_eq!(
            judge(&mut w, idle, true, &File::Absent, true, false, t),
            Verdict::Wait
        );
        assert_eq!(
            judge(&mut w, idle, true, &File::Old, true, false, t),
            Verdict::Wait
        );
    }

    #[test]
    fn an_unknown_agent_is_done_once_its_file_stays_good() {
        let t = Instant::now();
        let mut w = watch();
        let good = File::Good((5, 9));
        assert_eq!(
            judge(&mut w, None, true, &good, false, false, t),
            Verdict::Wait
        );
        assert_eq!(
            judge(&mut w, None, true, &good, false, false, t + STABLE / 2),
            Verdict::Wait
        );
        // It changed: the wait starts again.
        let newer = File::Good((6, 9));
        assert_eq!(
            judge(&mut w, None, true, &newer, false, false, t + STABLE),
            Verdict::Wait
        );
        assert_eq!(
            judge(
                &mut w,
                Some(AgentStatus::Unknown),
                true,
                &newer,
                false,
                false,
                t + STABLE * 2
            ),
            Verdict::Ready((6, 9))
        );
    }

    #[test]
    fn a_gone_pane_ends_the_run() {
        let t = Instant::now();
        let mut w = watch();
        assert_eq!(
            judge(&mut w, None, false, &File::Absent, false, false, t),
            Verdict::Wait
        );
        assert_eq!(
            judge(&mut w, None, false, &File::Absent, false, false, t + STABLE),
            Verdict::Gone
        );
        let mut w = watch();
        let good = File::Good((1, 2));
        judge(&mut w, None, false, &good, false, false, t);
        assert_eq!(
            judge(&mut w, None, false, &good, false, false, t + STABLE),
            Verdict::Ready((1, 2))
        );
    }

    #[test]
    fn the_agent_line_says_where_it_stands() {
        let now = 1_000 + 180;
        let r = run(None);
        assert_eq!(
            agent_line(&r, Stage::Discover, Some(AgentStatus::Working), None, now),
            "Claude Code working \u{b7} started 3m ago"
        );
        assert_eq!(
            agent_line(&run(Some(1_100)), Stage::Discover, None, None, now),
            "brief.md ready for review"
        );
        let mut nudged = r.clone();
        nudged.nudged = true;
        let w = Watch {
            missing: vec!["no Outcome section".into()],
            ..Watch::default()
        };
        assert_eq!(
            agent_line(&nudged, Stage::Discover, None, Some(&w), now),
            "nudged: no Outcome section"
        );
        let mut closed = r;
        closed.pane = None;
        assert_eq!(
            agent_line(&closed, Stage::Plan, None, None, now),
            "Claude Code closed \u{b7} started 3m ago"
        );
    }

    #[test]
    fn prompts_render_for_the_feature() {
        let f = feature("a", vec![repo("api", true), repo("web", false)], 0);
        let v = vars(
            &f,
            &PerProject::all("Acme".to_owned()),
            Path::new("/data/a"),
            Stage::Plan,
        );
        assert_eq!(v["artifact.path"], "/data/a/plan.md");
        assert_eq!(v["brief.path"], "/data/a/brief.md");
        assert_eq!(v["multi_repo"], "yes");
        assert!(v["repos"].contains("- web: /work/.features/bulk/web (context only)"));
        let text = prompts::render_builtin("plan", &v).unwrap();
        assert!(text.contains("Feature a") && text.contains("Acme"));
        assert_eq!(stage_key(Stage::AiReview), "ai-review");
    }

    #[test]
    fn prompts_name_every_projects_folder_and_repositories() {
        let mut f = feature("a", vec![repo("api", true)], 0);
        f.multi = false;
        let mut ui = repo("ui", false);
        ui.project = Some("admin".into());
        ui.worktree = "/admin/.features/a/ui".into();
        f.repos.push(ui);
        f.links.push(feature::Link {
            project: "admin".into(),
            root: "/admin".into(),
            dir: "/admin/.features/a".into(),
            multi: false,
        });
        let mut names = PerProject::all("Acme".to_owned());
        names.links.insert("admin".into(), "Admin".into());
        let v = vars(&f, &names, Path::new("/data/a"), Stage::Plan);
        assert_eq!(v["multi_repo"], "yes");
        assert_eq!(
            v["projects"],
            "- Acme, home: /work/.features/a\n- Admin: /admin/.features/a"
        );
        assert_eq!(
            v["repos"],
            "- api (project Acme): /work/.features/bulk/api (changed by this feature)\n\
             - admin:ui (project Admin): /admin/.features/a/ui (context only)"
        );
        let text = prompts::render_builtin("plan", &v).unwrap();
        assert!(text.contains("/admin/.features/a") && text.contains("admin:ui"));
        // One project, one repository: as before.
        let one = feature("b", vec![repo("api", true)], 0);
        let v = vars(&one, &names, Path::new("/data/b"), Stage::Plan);
        assert_eq!((v["repos"].as_str(), v["projects"].as_str()), ("api", ""));
        assert_eq!(v["multi_repo"], "");
    }

    #[test]
    fn prompts_name_the_repositories_left_out_and_that_the_graph_lacks_them() {
        let dir = std::env::temp_dir().join(format!("gerdr-prompt-others-{}", std::process::id()));
        let (api, web) = (dir.join("api"), dir.join("web"));
        std::fs::create_dir_all(api.join(".git")).unwrap();
        std::fs::create_dir_all(web.join(".git")).unwrap();
        let mut f = feature("a", vec![repo("api", true)], 0);
        f.root = dir.clone();
        f.multi = true;
        let names = PerProject::all("Acme".to_owned());
        let v = vars(&f, &names, Path::new("/data/a"), Stage::Plan);
        assert_eq!(v["other_repos"], format!("- web: {}", web.display()));
        let text = prompts::render_builtin("plan", &v).unwrap();
        assert!(text.contains(&v["other_repos"]));
        assert!(text.contains("Ctrl+Shift+L"));
        assert!(!text.contains("The graph doesn't hold them"));
        // With a graph: it says the graph doesn't hold them.
        std::fs::create_dir_all(api.join(graph::OUT_DIR)).unwrap();
        std::fs::write(graph::file(&api), "{}").unwrap();
        f.repos[0].main = api;
        let v = vars(&f, &names, Path::new("/data/a"), Stage::Plan);
        let text = prompts::render_builtin("plan", &v).unwrap();
        assert!(text.contains("The graph doesn't hold them"));
        assert!(text.contains("still being built"));
        // Every repository in: nothing to say.
        let web_repo = feature::Repo {
            main: web,
            ..repo("web", true)
        };
        f.repos.push(web_repo);
        let v = vars(&f, &names, Path::new("/data/a"), Stage::Plan);
        assert_eq!(v["other_repos"], "");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prompts_name_the_code_graphs_of_projects_that_keep_them() {
        let dir = std::env::temp_dir().join(format!("gerdr-prompt-graph-{}", std::process::id()));
        let main = dir.join("api");
        std::fs::create_dir_all(main.join(graph::OUT_DIR)).unwrap();
        std::fs::write(graph::file(&main), "{}").unwrap();
        let mut f = feature("a", vec![repo("api", true)], 0);
        f.multi = false;
        let names = PerProject::all("Acme".to_owned());
        let without = vars(&f, &names, Path::new("/data/a"), Stage::Plan);
        assert_eq!(without["graphs"], "");
        let text = prompts::render_builtin("plan", &without).unwrap();
        assert!(!text.contains("graphify"));
        f.repos[0].main = main;
        let with = vars(&f, &names, Path::new("/data/a"), Stage::Plan);
        let file = graph::file(&f.dir);
        assert_eq!(with["graphs"], format!("- {}", file.display()));
        let text = prompts::render_builtin("plan", &with).unwrap();
        assert!(text.contains("graphify query"));
        assert!(text.contains(&format!("from:\n\n{}\n\nAfter", with["graphs"])));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_build_agent_done_after_some_milestones_stops() {
        let t = Instant::now();
        let partial = File::Partial((5, 9), 2, 4);
        let mut w = watch();
        let idle = Some(AgentStatus::Idle);
        // Not before it worked.
        assert_eq!(
            judge(&mut w, idle, true, &partial, false, false, t),
            Verdict::Wait
        );
        assert_eq!(
            judge(
                &mut w,
                Some(AgentStatus::Working),
                true,
                &partial,
                false,
                false,
                t
            ),
            Verdict::Wait
        );
        let stop = Verdict::Stop { done: 2, total: 4 };
        assert_eq!(judge(&mut w, idle, true, &partial, false, false, t), stop);
        // A stop isn't a nudge, nudged before or not.
        assert_eq!(judge(&mut w, idle, true, &partial, false, true, t), stop);
        assert!(w.missing.is_empty());
        // herdr doesn't know it: once the log stays as it is.
        let mut w = watch();
        assert_eq!(
            judge(&mut w, None, true, &partial, false, false, t),
            Verdict::Wait
        );
        assert_eq!(
            judge(&mut w, None, true, &partial, false, false, t + STABLE),
            stop
        );
        // Its tab gone: stopped there too.
        let mut w = watch();
        judge(&mut w, None, false, &partial, false, false, t);
        assert_eq!(
            judge(&mut w, None, false, &partial, false, false, t + STABLE),
            stop
        );
    }

    #[test]
    fn an_unknown_agent_told_to_go_on_is_not_stopped_again() {
        let t = Instant::now();
        let mut w = Watch {
            went_on: Some(2),
            ..watch()
        };
        // Still at the milestone it went on from, however long.
        let partial = File::Partial((5, 9), 2, 4);
        judge(&mut w, None, true, &partial, false, false, t);
        assert_eq!(
            judge(&mut w, None, true, &partial, false, false, t + STABLE * 3),
            Verdict::Wait
        );
        // The next one ticked: a stop.
        let next = File::Partial((6, 9), 3, 4);
        judge(&mut w, None, true, &next, false, false, t + STABLE * 3);
        assert_eq!(
            judge(&mut w, None, true, &next, false, false, t + STABLE * 4),
            Verdict::Stop { done: 3, total: 4 }
        );
        // herdr seeing it rest after working still stops it where it is.
        let mut w = Watch {
            went_on: Some(2),
            worked: true,
            ..watch()
        };
        assert_eq!(
            judge(
                &mut w,
                Some(AgentStatus::Idle),
                true,
                &partial,
                false,
                false,
                t
            ),
            Verdict::Stop { done: 2, total: 4 }
        );
    }

    #[test]
    fn only_another_milestone_is_a_new_stop() {
        let stop = |done, total| Stop { done, total, at: 5 };
        assert!(stop_is_new(None, 1, 3));
        assert!(!stop_is_new(Some(stop(1, 3)), 1, 3));
        assert!(stop_is_new(Some(stop(1, 3)), 2, 3));
        assert!(stop_is_new(Some(stop(1, 3)), 1, 4));
        assert_eq!(
            partial(Stage::Plan, "## Milestones\n- [x] a\n- [ ] b\n"),
            None
        );
    }

    #[test]
    fn gates_wait_for_reviews_milestones_and_verify_failing_too_often() {
        let mut review = feature("a", vec![repo("api", true)], 1);
        review.runs.insert(Stage::Plan, run(Some(400)));
        // Build's ready says it was marked done: no review.
        let mut built = feature("b", vec![repo("api", true)], 2);
        built.runs.insert(Stage::Build, run(Some(100)));
        let mut stopped = feature("c", vec![repo("api", true)], 2);
        let mut r = run(None);
        r.stop = Some(Stop {
            done: 2,
            total: 4,
            at: 300,
        });
        stopped.runs.insert(Stage::Build, r);
        let failed = |slug: &str, fixes: u32| {
            let mut f = feature(slug, vec![repo("api", true)], 3);
            f.verify = Some(verify::VerifyRun {
                n: 4,
                started: 150,
                finished: Some(200),
                passed: Some(false),
                fixes,
                ..verify::VerifyRun::default()
            });
            f
        };
        let all = BTreeMap::from([(
            "one".to_owned(),
            vec![review, built, stopped, failed("d", 3), failed("e", 2)],
        )]);
        let gates = gates(&all, |_| None);
        let got: Vec<(&str, Stage, u64, GateKind)> = gates
            .iter()
            .map(|g| (g.slug.as_str(), g.stage, g.since, g.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("d", Stage::Verify, 200, GateKind::VerifyFailed { fixes: 3 }),
                (
                    "c",
                    Stage::Build,
                    300,
                    GateKind::Milestone { done: 2, total: 4 }
                ),
                ("a", Stage::Plan, 400, GateKind::Review),
            ]
        );
    }

    #[test]
    fn the_build_agent_line_says_milestones_checks_and_what_is_missing() {
        let now = 1_000 + 180;
        let mut r = run(None);
        let checking = Watch {
            checking: Some((1, 1)),
            ..Watch::default()
        };
        assert_eq!(
            agent_line(&r, Stage::Build, None, Some(&checking), now),
            "checking the code"
        );
        let refused = Watch {
            refused: Some(((1, 1), "uncommitted changes in api".into())),
            ..Watch::default()
        };
        assert_eq!(
            agent_line(&r, Stage::Build, None, Some(&refused), now),
            "not ready: uncommitted changes in api"
        );
        // Ready from before its code changed isn't a review.
        r.ready = Some(1_100);
        assert_eq!(
            agent_line(&r, Stage::Build, Some(AgentStatus::Idle), None, now),
            "Claude Code waiting for you \u{b7} started 3m ago"
        );
        r.stop = Some(Stop {
            done: 2,
            total: 4,
            at: 1_150,
        });
        assert_eq!(
            agent_line(&r, Stage::Build, None, Some(&checking), now),
            "milestone 2 of 4 done \u{b7} Enter to continue"
        );
    }

    #[test]
    fn the_build_prompt_gets_the_plan_evidence_and_branch() {
        let f = feature("a", vec![repo("api", true)], 2);
        let v = vars(
            &f,
            &PerProject::all("Acme".to_owned()),
            Path::new("/data/a"),
            Stage::Build,
        );
        assert_eq!(v["artifact.path"], "/data/a/build.md");
        assert_eq!(v["plan.path"], "/data/a/plan.md");
        assert_eq!(v["evidence.path"], "/data/a/evidence.md");
        assert_eq!(v["evidence"], "");
        assert_eq!(v["branch"], "feature/a");
        let text = prompts::render_builtin("build", &v).unwrap();
        assert!(text.contains("Feature a") && !text.contains("npm test"));
        let mut fix = v.clone();
        fix.insert("evidence".into(), "api: `npm test` exited with 1".into());
        let text = prompts::render_builtin("build", &fix).unwrap();
        assert!(text.contains("npm test"));
        let line = prompts::render_builtin("fix", &v).unwrap();
        assert!(line.contains("/data/a/evidence.md") && !line.trim().contains('\n'));
        let line = prompts::render_builtin("continue", &v).unwrap();
        assert!(line.contains("/data/a/build.md") && !line.trim().contains('\n'));
    }

    fn claude_and_cursor() -> [CliAgent; 2] {
        [
            agent("Claude Code", Start::Command("claude".into())),
            agent("Cursor", Start::Command("cursor-agent".into())),
        ]
    }

    fn with_agents(entries: &[(Stage, &str)]) -> Workflow {
        Workflow {
            agents: entries.iter().map(|(s, a)| (*s, (*a).to_owned())).collect(),
            ..Workflow::default()
        }
    }

    fn who(pick: Pick) -> String {
        match pick {
            Pick::Agent(name, _) => name,
            other => panic!("{other:?}"),
        }
    }

    fn lit(chips: &[AgentChip]) -> Vec<&str> {
        let on = chips.iter().filter(|c| c.lit);
        on.map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn a_pick_beats_the_project_file_which_beats_the_first_installed() {
        let installed = claude_and_cursor();
        let mut f = feature("a", vec![repo("r", true)], 0);
        let plan = Stage::Plan;
        assert_eq!(who(stage_pick(&f, plan, None, &installed)), "Claude Code");
        let set = with_agents(&[(plan, "Cursor")]);
        assert_eq!(who(stage_pick(&f, plan, Some(&set), &installed)), "Cursor");
        f.stage_agents.insert(plan, "Claude Code".into());
        let picked = stage_pick(&f, plan, Some(&set), &installed);
        assert_eq!(who(picked), "Claude Code");
    }

    #[test]
    fn a_pick_since_uninstalled_warns_and_runs_no_command() {
        let installed = [agent("Claude Code", Start::Command("claude".into()))];
        let mut f = feature("a", vec![repo("r", true)], 0);
        f.stage_agents.insert(Stage::Plan, "Cursor".into());
        let Pick::Refused(why) = stage_pick(&f, Stage::Plan, None, &installed) else {
            panic!("a name isn't a command");
        };
        assert!(
            why.contains("Cursor") && why.contains("isn't installed"),
            "{why}"
        );
        let chips = stage_chips(&f, Stage::Plan, None, &installed);
        assert!(lit(&chips).is_empty(), "{chips:?}");
    }

    #[test]
    fn the_chips_offer_claude_and_cursor_and_light_who_starts() {
        let mut installed = claude_and_cursor().to_vec();
        installed.insert(0, agent("Codex", Start::Command("codex".into())));
        installed.insert(0, agent("T3 Code", Start::T3));
        let mut f = feature("a", vec![repo("r", true)], 0);
        let names =
            |chips: &[AgentChip]| -> Vec<String> { chips.iter().map(|c| c.name.clone()).collect() };
        // Codex comes first of the commands, so it is who would start.
        let chips = stage_chips(&f, Stage::Build, None, &installed);
        assert_eq!(names(&chips), ["T3 Code", "Claude Code", "Cursor", "Codex"]);
        assert_eq!(lit(&chips), ["Codex"]);
        f.stage_agents.insert(Stage::Build, "Cursor".into());
        let chips = stage_chips(&f, Stage::Build, None, &installed);
        assert_eq!(lit(&chips), ["Cursor"]);
    }

    #[test]
    fn t3_code_starts_only_when_picked_and_no_flag_is_its_command() {
        let installed = [
            agent("T3 Code", Start::T3),
            agent("Claude Code", Start::Command("claude".into())),
        ];
        let mut f = feature("a", vec![repo("r", true)], 0);
        let claude = Pick::Agent("Claude Code".into(), "claude".into());
        assert_eq!(stage_pick(&f, Stage::Plan, None, &installed), claude);
        f.stage_agents.insert(Stage::Plan, "T3 Code".into());
        let t3 = Pick::Agent("T3 Code".into(), "t3".into());
        assert_eq!(stage_pick(&f, Stage::Plan, None, &installed), t3);
        // Set by the project's file too, by its name: no shell line "T3 Code".
        assert_eq!(
            pick_agent(Some("t3 code"), &installed),
            Some(("T3 Code".into(), "t3".into()))
        );
    }

    #[test]
    fn t3_code_reviews_what_another_of_its_agents_built() {
        let installed = [
            agent("T3 Code", Start::T3),
            agent("Claude Code", Start::Command("claude".into())),
            agent("Cursor", Start::Command("cursor-agent".into())),
        ];
        let mut f = feature("a", vec![repo("r", true)], 0);
        let built_in_t3 = |agent: Option<&str>| StageRun {
            agent: "T3 Code".into(),
            command: "t3".into(),
            t3: Some(T3Run::Thread {
                id: "t".into(),
                agent: agent.map(str::to_owned),
            }),
            ..StageRun::default()
        };
        let off = |f: &Feature| -> Vec<String> {
            let chips = stage_chips(f, Stage::AiReview, None, &installed);
            chips
                .into_iter()
                .filter(|c| c.off)
                .map(|c| c.name)
                .collect()
        };
        // Which agent it ran isn't known yet: none is told apart.
        f.runs.insert(Stage::Build, built_in_t3(None));
        assert!(off(&f).is_empty());
        // Claude built it through T3 Code: Claude Code may not review it,
        // but T3 Code may, by another of its agents.
        f.runs.insert(Stage::Build, built_in_t3(Some("claude")));
        assert_eq!(off(&f), ["Claude Code"]);
        f.runs.insert(Stage::Build, built_in_t3(Some("cursor")));
        assert_eq!(off(&f), ["Cursor"]);
        // Picked in the draft, the same agent can only be told of after.
        let builder = t3_reviews_its_build(&f, None, &installed, "cursor");
        assert_eq!(builder.as_deref(), Some("cursor"));
        assert_eq!(t3_reviews_its_build(&f, None, &installed, "claude"), None);
    }

    #[test]
    fn a_draft_says_so_and_waits_for_its_thread() {
        let run = StageRun {
            agent: "T3 Code".into(),
            t3: Some(T3Run::Draft),
            ..StageRun::default()
        };
        assert!(run.live());
        assert!(run.t3_thread().is_none());
        assert!(run.agent_key().is_none());
        let line = agent_line(&run, Stage::Plan, None, None, 0);
        assert!(line.contains("pick a model"), "{line}");
    }

    #[test]
    fn the_project_files_own_command_gets_a_chip_of_its_own() {
        let installed = claude_and_cursor();
        let f = feature("a", vec![repo("r", true)], 0);
        let set = with_agents(&[(Stage::Plan, "opencode --prompt")]);
        let chips = stage_chips(&f, Stage::Plan, Some(&set), &installed);
        assert_eq!(chips.len(), 3);
        assert_eq!(lit(&chips), ["opencode"]);
    }

    #[test]
    fn only_stages_still_ahead_with_an_agent_have_chips() {
        let installed = claude_and_cursor();
        let mut f = feature("a", vec![repo("r", true)], 2);
        assert!(stage_chips(&f, Stage::Discover, None, &installed).is_empty());
        assert_eq!(stage_chips(&f, Stage::Build, None, &installed).len(), 2);
        assert_eq!(stage_chips(&f, Stage::AiReview, None, &installed).len(), 2);
        assert!(stage_chips(&f, Stage::Verify, None, &installed).is_empty());
        f.status = Status::Abandoned;
        assert!(stage_chips(&f, Stage::Build, None, &installed).is_empty());
    }

    #[test]
    fn the_builder_can_review_only_when_the_project_allows_it() {
        let installed = claude_and_cursor();
        let mut f = feature("a", vec![repo("r", true)], 0);
        // Build hasn't run: who it will start counts.
        f.stage_agents.insert(Stage::Build, "Cursor".into());
        let chips = stage_chips(&f, Stage::AiReview, None, &installed);
        assert_eq!(
            chips.iter().map(|c| c.off).collect::<Vec<_>>(),
            [false, true]
        );
        assert_eq!(lit(&chips), ["Claude Code"]);
        // Its run counts over any pick.
        let run = StageRun {
            agent: "Claude Code".into(),
            command: "claude --add-dir /d -- \"$(cat /d/p.md)\"".into(),
            ..StageRun::default()
        };
        f.runs.insert(Stage::Build, run);
        let chips = stage_chips(&f, Stage::AiReview, None, &installed);
        assert_eq!(
            chips.iter().map(|c| c.off).collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(lit(&chips), ["Cursor"]);
        // Picked before Build ran, then refused at the start.
        f.stage_agents.insert(Stage::AiReview, "Claude Code".into());
        let refused = stage_pick(&f, Stage::AiReview, None, &installed);
        assert!(matches!(refused, Pick::Refused(_)), "{refused:?}");
        let allowed = Workflow {
            allow_same_reviewer: true,
            ..Workflow::default()
        };
        let chips = stage_chips(&f, Stage::AiReview, Some(&allowed), &installed);
        assert!(chips.iter().all(|c| !c.off), "{chips:?}");
        let picked = stage_pick(&f, Stage::AiReview, Some(&allowed), &installed);
        assert_eq!(who(picked), "Claude Code");
    }

    #[test]
    fn the_next_chip_skips_the_lit_and_the_off_and_goes_round() {
        let chip = |name: &str, lit, off| AgentChip {
            icon: AgentIcon::from_agent(name),
            name: name.into(),
            lit,
            off,
        };
        let name = |chips: &[AgentChip]| next_chip(chips).map(|c| c.name.clone());
        let two = [chip("a", true, false), chip("b", false, false)];
        assert_eq!(name(&two).as_deref(), Some("b"));
        let back = [chip("a", false, false), chip("b", true, false)];
        assert_eq!(name(&back).as_deref(), Some("a"));
        let none = [chip("a", true, false), chip("b", false, true)];
        assert_eq!(name(&none), None);
        let unlit = [chip("a", false, true), chip("b", false, false)];
        assert_eq!(name(&unlit).as_deref(), Some("b"));
        assert_eq!(next_chip(&[]), None);
    }
}
