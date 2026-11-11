//! AI work: everything the agents of a feature did, kept for good, to look
//! back at and to learn from. It lives apart from the feature, in
//! `<data>/ai-work/<project>/<slug>/`, so it outlasts the feature's removal
//! and no copy between machines touches it:
//!
//! - `events-<machine>.jsonl`: what happened, a `Line` each, appended only
//!   by that machine;
//! - `record.json`: the latest record, `cursor.json`: how far it was taken;
//! - `blobs/<aa>/<sha256>.zst`: prompts, files sent, stage files, check
//!   logs and setup logs, by their content (see `blobs`);
//! - `sessions/<agent>-<id>/`: each chat's transcript, copied from the
//!   agent CLI's own store (see `locate`) and compressed (see `zst`).
//!
//! The files are what counts. `index.sqlite` beside the projects is drawn
//! from them (see `index`) for the stats panel (see `stats`), and is drawn
//! again when it is gone.
//!
//! What happens to the record shows as the difference between one save of
//! it and the next (see `observe`). What the record never holds comes in
//! by hand: the lines typed into agents (see `agent_tell`), the check logs
//! and the transcripts. The heavy work runs on its own thread (see
//! `thread`).

pub mod backfill;
pub mod blobs;
pub mod index;
pub mod locate;
pub mod observe;
pub mod stats;
pub mod thread;
pub mod usage;
pub mod write;
pub mod zst;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use smithay::reexports::calloop::channel;

use crate::{Gerdr, feature::Stage, features::Done};

/// The version of the lines and files written. A reader skips what is newer
/// than it knows.
pub const FORMAT: u32 = 1;

/// The folder of all AI work under data folder `data`.
pub fn root(data: &Path) -> PathBuf {
    data.join("ai-work")
}

/// The folder of the AI work on feature `slug` of `project`.
pub fn dir(data: &Path, project: &str, slug: &str) -> PathBuf {
    root(data).join(project).join(slug)
}

/// What this machine is called in the files it writes: its id among the
/// machines, or "here" while it has none. `Live::start` makes the id, so
/// two machines never both write `events-here.jsonl` from then on.
pub fn machine() -> String {
    crate::machine_views::own_id().unwrap_or_else(|| "here".to_owned())
}

/// How many of `ids`, the findings an AI review's file holds, AI review
/// `n` raised: those named `r<n>-<k>`. Those carried from earlier reviews
/// and the user's own (`u…`) aren't its.
pub fn raised_by<'a>(n: u32, ids: impl IntoIterator<Item = &'a str>) -> usize {
    let prefix = format!("r{n}-");
    ids.into_iter().filter(|id| id.starts_with(&prefix)).count()
}

/// A file kept by its content: `blobs/<aa>/<sha>.zst` of a feature's
/// folder, `aa` the first two digits of `sha`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlobRef {
    /// The SHA-256 of the content, in hex.
    pub sha: String,
    /// The content's length, before it was compressed.
    pub len: u64,
}

impl BlobRef {
    /// The reference of `bytes`, kept or not.
    pub fn of(bytes: &[u8]) -> Self {
        let sha = Sha256::digest(bytes);
        let sha = sha.iter().map(|b| format!("{b:02x}")).collect::<String>();
        Self {
            sha,
            len: bytes.len() as u64,
        }
    }

    /// Where it is kept in feature folder `dir`.
    pub fn path(&self, dir: &Path) -> PathBuf {
        let aa = self.sha.get(..2).unwrap_or("00");
        dir.join("blobs").join(aa).join(format!("{}.zst", self.sha))
    }
}

/// One line of `events-<machine>.jsonl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// `FORMAT` when it was written.
    pub v: u32,
    /// When it happened, in milliseconds since the epoch.
    pub at_ms: u64,
    /// Which machine wrote it (see `machine`).
    pub machine: String,
    /// It was made later from what was left (see `backfill`), not seen.
    #[serde(default, skip_serializing_if = "is_false")]
    pub backfill: bool,
    #[serde(flatten)]
    pub event: Event,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Line {
    /// `event`, seen now on this machine.
    pub fn now(event: Event) -> Self {
        Self {
            v: FORMAT,
            at_ms: now_ms(),
            machine: machine(),
            backfill: false,
            event,
        }
    }
}

/// The time now, in milliseconds since the epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// What happened. Pieces of the record are kept as JSON as they were, so a
/// later change to the record's types never breaks an old line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    // From the record (see `observe`).
    /// Line `i` of the feature's history: `what` at `at`.
    Note { i: usize, at: u64, what: String },
    /// Run `n` of `stage` began, `how`, on prompt `prompt`.
    RunStarted {
        stage: Stage,
        n: u32,
        run: Value,
        prompt: Option<BlobRef>,
        how: StartHow,
    },
    /// Something of run `n` of `stage` changed.
    RunChanged {
        stage: Stage,
        n: u32,
        change: RunChange,
    },
    /// Run `n` of `stage` is over, `why`; `run` as it was last.
    RunEnded {
        stage: Stage,
        n: u32,
        why: EndWhy,
        run: Value,
    },
    /// `stage` was marked done.
    Marked { stage: Stage, mark: Value },
    /// `stage`'s mark was taken back.
    Unmarked { stage: Stage },
    /// The feature started over, from `stage`.
    StartedOver { stage: Option<Stage> },
    /// The feature's status went from `from` to `to`.
    Status { from: Value, to: Value },
    /// Fix round `n` opened.
    FixRoundOpened { n: u32, round: Value },
    /// Fix round `n` closed.
    FixRoundClosed { n: u32, round: Value },
    /// AI review `n` was read: `findings` of them, all in `review`.
    ReviewRead {
        n: u32,
        findings: usize,
        review: Option<BlobRef>,
    },
    /// Plan review `n` was read: `suggestions` of them, all in `review`.
    PlanReviewRead {
        n: u32,
        suggestions: usize,
        review: Option<BlobRef>,
    },
    /// Verify run `run` (`v{n}@{started}`: numbers restart after a start
    /// over) began.
    VerifyStarted { run: String, verify: Value },
    /// Check `i` (`key`) of Verify run `run` ended; what it printed is in
    /// `log`, from its start unless `truncated`.
    CheckEnded {
        run: String,
        key: String,
        i: usize,
        outcome: Value,
        log: Option<BlobRef>,
        #[serde(default)]
        truncated: bool,
    },
    /// Verify run `run` ended.
    VerifyEnded { run: String, verify: Value },
    /// A setup ended; what it printed is in `log`.
    SetupEnded { run: Value, log: Option<BlobRef> },
    /// The manual test sheet changed.
    Manual { manual: Value },
    /// The whole record, now.
    Snapshot { record: BlobRef },

    // From the lines typed into agents (see `agent_tell`).
    /// `text` was typed into run `n` of `stage`, for `send` (`kind` is the
    /// event's tag), through `via`; `files`: the data folder files it
    /// names, by their path then.
    Sent {
        stage: Stage,
        n: u32,
        send: SendKind,
        text: String,
        #[serde(default)]
        files: BTreeMap<String, BlobRef>,
        via: Via,
    },
    /// `text` couldn't be typed into run `n` of `stage`: `why`. `went`: it
    /// was taken at once, its `Sent` kept, and failed after.
    SendFailed {
        stage: Stage,
        n: u32,
        send: SendKind,
        text: String,
        via: Via,
        why: String,
        #[serde(default, skip_serializing_if = "is_false")]
        went: bool,
    },

    // From the thread (see `thread`).
    /// Stage file `file` as it was, kept in `blob`, `reason`.
    Output {
        file: String,
        blob: BlobRef,
        reason: String,
    },
    /// The transcript of chat `session` of `agent` was kept, for `runs`;
    /// `raw_bytes` of it, all of it unless not `complete`.
    Transcript {
        agent: AgentKind,
        session: String,
        runs: Vec<(Stage, u32)>,
        raw_bytes: u64,
        complete: bool,
    },
    /// The chat of run `n` of `stage` couldn't be found: `why`.
    TranscriptMissing {
        agent: AgentKind,
        session: Option<String>,
        stage: Stage,
        n: u32,
        why: String,
    },
}

/// How a run began.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartHow {
    /// In a chat of its own.
    Fresh,
    /// In the chat of an agent that waited for new work.
    Reused,
    /// In a closed chat taken up again.
    Resumed,
    /// Going on after a milestone stop.
    Milestone,
}

/// What changed of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunChange {
    PromptSent,
    Nudged,
    Model,
    Ready,
    SentBack,
    Stop,
    Blocked,
    Unblocked,
    PaneLost,
    /// Its agent left its tab, the tab still there (see `exited_line`).
    Exited,
    T3Thread,
}

/// The history line saying run `n` of `stage`'s agent left its tab.
pub fn exited_line(stage: crate::feature::Stage, n: u32) -> String {
    format!("{} agent exited (run {n})", stage.label())
}

/// Why a run is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndWhy {
    /// A new run of the stage took its place.
    Replaced,
    /// The record forgot it.
    Forgotten,
    /// The feature started over.
    StartedOver,
    /// The feature was closed.
    FeatureClosed,
}

/// What a line typed into a stage's agent is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendKind {
    /// The stage's prompt.
    Start,
    /// The user's comments, sent back.
    SendBack,
    /// The notes on the plan review's suggestions, sent back.
    PlanReviewBack,
    /// Its file isn't there, or lacks something.
    Nudge,
    /// Compact the chat before a fix round.
    Compact,
    /// Plan the fixes.
    FixPlan,
    /// Make the next fix.
    FixOn,
    /// Make the fixes.
    FixGo,
    /// Build isn't done: why.
    BuildCheck,
    /// Go on after a milestone stop.
    Continue,
}

/// How a line reached the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// A herdr pane here.
    Pane,
    /// A pane on another machine.
    Machine,
    /// A T3 Code thread.
    T3,
}

/// Which agent CLI a chat is of; also the start of its `sessions/` folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Cursor,
    Codex,
    OpenCode,
    T3,
    Unknown,
}

impl AgentKind {
    /// The kind of agent `name` (as `StageRun::agent` holds it).
    pub fn of(name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            n if n.starts_with("claude") => Self::Claude,
            n if n.starts_with("cursor") => Self::Cursor,
            n if n.starts_with("codex") => Self::Codex,
            n if n.starts_with("opencode") => Self::OpenCode,
            n if n.starts_with("t3") => Self::T3,
            _ => Self::Unknown,
        }
    }

    /// Its name in folder names: `sessions/<name>-<id>/`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::T3 => "t3",
            Self::Unknown => "unknown",
        }
    }
}

/// A chat of a stage's agent, to find and keep (see `locate`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chat {
    pub agent: AgentKind,
    /// Its id, once known: Codex's and OpenCode's are found by folder and
    /// time.
    pub id: Option<String>,
    pub stage: Stage,
    pub n: u32,
    /// When the run began, in seconds since the epoch.
    pub started: u64,
    /// The feature's folders: where an agent without an id ran.
    pub folders: Vec<PathBuf>,
    /// The machine it runs on, `None` here.
    pub machine: Option<String>,
}

/// What the thread came to, back on the loop as `Done::AiWork`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// A write or read worked again at `at`: the problem said there goes.
    Worked { at: String },
    /// `what` failed at `at` (a feature, or a part of the thread): `why`,
    /// said as a problem there.
    Failed {
        at: String,
        what: String,
        why: String,
    },
    /// The index took in `lines` new lines.
    Indexed { lines: usize },
    /// Keeping earlier AI work: `done` of `total` features.
    Backfilling { done: usize, total: usize },
    /// Earlier AI work kept for `features` features; `gone` of their chats
    /// were gone already. `stopped`: asked to stop before the end.
    Backfilled {
        features: usize,
        gone: usize,
        stopped: bool,
    },
    /// Drawing the index again from the files: `done` of `total` features.
    Rescanning { done: usize, total: usize },
    /// The index was drawn again, with `lines` lines; `stopped`: asked to
    /// stop before the end.
    Rescanned { lines: usize, stopped: bool },
    /// The AI work of feature `title`, taken over from machine `from`,
    /// couldn't be brought here: `why`. It stays there.
    NotBrought {
        title: String,
        from: String,
        why: String,
    },
    /// Bringing the AI work of feature `slug` of `project` is over, as it
    /// went.
    PullEnded { project: String, slug: String },
    /// The AI work of feature `title` was deleted, or couldn't be: `why`.
    Deleted { title: String, why: Option<String> },
}

/// What the thread works at that the tasks panel shows, and can stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    /// Keeping earlier AI work: `done` of `total` features.
    Backfill { done: usize, total: usize },
    /// Drawing the index again: `done` of `total` features.
    Rescan { done: usize, total: usize },
}

/// The AI work of this gerdr: where it goes, the thread doing the heavy
/// part, and how each feature's record was last seen.
pub struct Live {
    /// `ai-work` of the data folder; `None` while off.
    root: Option<PathBuf>,
    /// The thread's jobs (see `thread`), once it runs.
    jobs: Option<mpsc::Sender<thread::Job>>,
    /// Each feature's record as last saved, by project and slug.
    pub seen: HashMap<(String, String), observe::Digest>,
    /// This machine, in the lines it writes.
    machine: String,
    /// Set to stop a backfill or a rescan, the thread going on.
    cancel: Arc<AtomicBool>,
    /// What the thread works at, for the tasks panel.
    pub busy: Option<Busy>,
    /// It was asked to stop what `busy` says.
    pub stopping: bool,
    /// The AI work being brought from another machine, by project and slug.
    pub pulls: BTreeMap<(String, String), Pulling>,
}

/// The AI work of a feature taken over, coming from the machine that wrote
/// it, on a thread of its own.
#[derive(Debug)]
pub struct Pulling {
    /// The feature's title.
    pub title: String,
    /// The name of the machine it comes from.
    pub from: String,
    /// Set to stop it: by the tasks panel, or as gerdr quits.
    pub stop: Arc<AtomicBool>,
}

impl Live {
    /// Nothing is kept. Tests use it, so they never write to the real data
    /// folder.
    pub fn off() -> Self {
        Self {
            root: None,
            jobs: None,
            seen: HashMap::new(),
            machine: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: None,
            stopping: false,
            pulls: BTreeMap::new(),
        }
    }

    /// Keep AI work under data folder `data`, the thread telling `done`
    /// what it came to. Without its thread the lines are still written,
    /// and the problem is said.
    pub(crate) fn start(data: &Path, done: channel::Sender<Done>) -> Self {
        // The lines name this machine: an id first, or each machine's
        // would be `here`, and one would go over another's.
        if data == crate::projects::data_dir() {
            crate::machine_lease::own_id_made();
        }
        let root = root(data);
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let spawned = {
            let (root, cancel) = (root.clone(), cancel.clone());
            std::thread::Builder::new()
                .name("ai-work".into())
                .spawn(move || thread::run(root, rx, done, cancel))
        };
        let jobs = match spawned {
            Ok(_) => Some(tx),
            Err(err) => {
                tracing::warn!("ai-work: starting its thread: {err}");
                crate::problems::report(
                    THREAD_PROBLEM,
                    format!("Couldn't start keeping the AI work's chats and stats: {err}"),
                );
                None
            }
        };
        Self {
            root: Some(root),
            jobs,
            seen: HashMap::new(),
            machine: machine(),
            cancel,
            busy: None,
            stopping: false,
            pulls: BTreeMap::new(),
        }
    }

    /// Where the thread's jobs go, for another thread to give it some.
    pub fn sender(&self) -> Option<mpsc::Sender<thread::Job>> {
        self.jobs.clone()
    }

    /// Stop the backfill or the rescan under way; the thread goes on.
    pub fn stop_busy(&mut self) {
        if self.busy.is_some() {
            self.cancel.store(true, Ordering::Relaxed);
            self.stopping = true;
        }
    }

    /// Stop bringing the AI work of feature `slug` of `project`.
    pub fn stop_pull(&mut self, project: &str, slug: &str) {
        if let Some(pull) = self.pulls.get(&(project.to_owned(), slug.to_owned())) {
            pull.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Stop every pull: gerdr quits.
    pub fn stop_pulls(&self) {
        for pull in self.pulls.values() {
            pull.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Whether anything is kept.
    pub fn is_on(&self) -> bool {
        self.root.is_some()
    }

    /// `ai-work` of the data folder, while on.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The folder of the AI work on feature `slug` of `project`, while on.
    pub fn dir(&self, project: &str, slug: &str) -> Option<PathBuf> {
        Some(self.root.as_ref()?.join(project).join(slug))
    }

    /// This machine, in the lines it writes.
    pub fn machine(&self) -> &str {
        &self.machine
    }

    /// Give the thread `job`. Off, or the thread gone, it is dropped.
    pub fn send(&self, job: thread::Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }
}

/// The problems the thread says go under this source. The writes on the
/// loop say theirs under `"ai-work"`, so neither clears the other's.
pub const THREAD_PROBLEM: &str = "ai-work-thread";

/// What the toast says once earlier AI work was kept.
fn backfilled_text(features: usize, gone: usize, stopped: bool) -> String {
    let kept = match features {
        1 => "Kept the AI work of 1 feature".to_owned(),
        n => format!("Kept the AI work of {n} features"),
    };
    let gone = match gone {
        0 => String::new(),
        1 => "; 1 chat was already gone".to_owned(),
        n => format!("; {n} chats were already gone"),
    };
    let stopped = match stopped {
        true => "; the rest is kept when gerdr starts again",
        false => "",
    };
    format!("{kept}{gone}{stopped}")
}

impl Gerdr {
    /// What the AI work's thread came to.
    pub(crate) fn ai_work_done(&mut self, report: Report) {
        match report {
            Report::Worked { at } => crate::problems::clear_at(THREAD_PROBLEM, &at),
            Report::Failed { at, what, why } => {
                crate::problems::report_at(THREAD_PROBLEM, &at, format!("{what}: {why}"));
            }
            Report::Indexed { .. } => self.ai_stats_refresh(),
            Report::Backfilling { done, total } => {
                self.ai_work.busy = Some(Busy::Backfill { done, total });
            }
            Report::Backfilled {
                features,
                gone,
                stopped,
            } => {
                self.ai_work.busy = None;
                self.ai_work.stopping = false;
                if features == 0 && gone == 0 && !stopped {
                    return;
                }
                let text = backfilled_text(features, gone, stopped);
                match gone > 0 {
                    true => self.show_warning(text),
                    false => self.show_success(text),
                }
            }
            Report::NotBrought { title, from, why } => self.show_warning(format!(
                "The AI work of \u{201c}{title}\u{201d} stays on {from}: {why}"
            )),
            Report::Deleted { title, why: None } => {
                self.show_success(format!("Deleted the AI work of \u{201c}{title}\u{201d}"));
                self.ai_stats_refresh();
            }
            Report::Deleted {
                title,
                why: Some(why),
            } => {
                self.show_error(format!(
                    "Couldn't delete the AI work of \u{201c}{title}\u{201d}: {why}"
                ));
                self.ai_stats_refresh();
            }
            Report::PullEnded { project, slug } => {
                self.ai_work.pulls.remove(&(project, slug));
            }
            Report::Rescanning { done, total } => {
                self.ai_work.busy = Some(Busy::Rescan { done, total });
            }
            Report::Rescanned { stopped, .. } => {
                self.ai_work.busy = None;
                self.ai_work.stopping = false;
                if stopped {
                    self.show_warning(
                        "Stopped drawing the AI work's stats again: the rest comes in when gerdr starts again".into(),
                    );
                }
                self.ai_stats_refresh();
            }
        }
    }

    /// Keep the earlier AI work of every feature this machine writes, once:
    /// the thread passes over those kept already.
    pub(crate) fn ai_work_backfill(&mut self) {
        if !self.ai_work.is_on() {
            return;
        }
        let features: Vec<(String, String)> = (self.features.all.iter())
            .flat_map(|(project, fs)| {
                (fs.iter().filter(|f| f.writer.is_none()))
                    .map(move |f| (project.clone(), f.slug.clone()))
            })
            .collect();
        if !features.is_empty() {
            self.ai_work.send(thread::Job::Backfill { features });
        }
    }

    /// The AI work's tasks: a backfill or a rescan under way, and the AI
    /// work coming from other machines.
    pub(crate) fn ai_work_tasks(&self) -> Vec<crate::tasks::Task> {
        let mut tasks: Vec<crate::tasks::Task> = (self.ai_work.pulls.iter())
            .map(|((project, slug), pull)| {
                let stopping = pull.stop.load(Ordering::Relaxed);
                crate::tasks::Task {
                    title: format!(
                        "Bringing the AI work of \u{201c}{}\u{201d} from {}",
                        pull.title, pull.from
                    ),
                    chip: "ai work".to_owned(),
                    detail: match stopping {
                        true => "Stopping".to_owned(),
                        false => "Its chats, prompts and stats, kept there".to_owned(),
                    },
                    dot: crate::chrome::Dot::Working,
                    actions: match stopping {
                        true => Vec::new(),
                        false => vec![(
                            "Stop",
                            crate::menu::MenuAction::StopAiWorkPull(project.clone(), slug.clone()),
                        )],
                    },
                }
            })
            .collect();
        let Some(busy) = self.ai_work.busy else {
            return tasks;
        };
        let (title, detail, done, total) = match busy {
            Busy::Backfill { done, total } => (
                "Keeping earlier AI work",
                "Once per feature: its record, prompts, files and the chats still there",
                done,
                total,
            ),
            Busy::Rescan { done, total } => (
                "Drawing the AI work's stats again",
                "From the files kept of each feature",
                done,
                total,
            ),
        };
        let stopping = self.ai_work.stopping;
        let detail = match stopping {
            true => "Stopping".to_owned(),
            false => detail.to_owned(),
        };
        let actions = match stopping {
            true => Vec::new(),
            false => vec![("Stop", crate::menu::MenuAction::StopAiWork)],
        };
        tasks.push(crate::tasks::Task {
            title: format!("{title} ({done} of {total})"),
            chip: "ai work".to_owned(),
            detail,
            dot: crate::chrome::Dot::Working,
            actions,
        });
        tasks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_flat_json_tagged_by_kind() {
        let line = Line {
            v: FORMAT,
            at_ms: 5,
            machine: "here".into(),
            backfill: false,
            event: Event::RunChanged {
                stage: Stage::Build,
                n: 2,
                change: RunChange::PaneLost,
            },
        };
        let text = serde_json::to_string(&line).unwrap();
        assert_eq!(
            text,
            r#"{"v":1,"at_ms":5,"machine":"here","kind":"run_changed","stage":"build","n":2,"change":"pane_lost"}"#
        );
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), line);
    }

    #[test]
    fn a_blob_is_kept_by_its_sha() {
        let blob = BlobRef::of(b"abc");
        assert_eq!(
            blob.sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(blob.len, 3);
        assert_eq!(
            blob.path(Path::new("/d")),
            Path::new("/d/blobs/ba").join(format!("{}.zst", blob.sha))
        );
    }

    #[test]
    fn the_backfill_toast_counts_what_it_says() {
        assert_eq!(
            backfilled_text(1, 0, false),
            "Kept the AI work of 1 feature"
        );
        assert_eq!(
            backfilled_text(41, 1, false),
            "Kept the AI work of 41 features; 1 chat was already gone"
        );
        assert_eq!(
            backfilled_text(3, 2, true),
            "Kept the AI work of 3 features; 2 chats were already gone; the rest is kept when gerdr starts again"
        );
    }

    #[test]
    fn agent_kinds_name_their_folders() {
        assert_eq!(AgentKind::of("claude"), AgentKind::Claude);
        assert_eq!(AgentKind::of("opencode"), AgentKind::OpenCode);
        assert_eq!(
            serde_json::to_string(&AgentKind::OpenCode).unwrap(),
            "\"opencode\""
        );
        assert_eq!(AgentKind::OpenCode.name(), "opencode");
    }
}
