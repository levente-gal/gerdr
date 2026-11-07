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

#![allow(dead_code)]

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
    sync::mpsc,
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
/// machines, or "here" while it has none.
pub fn machine() -> String {
    crate::machine_views::own_id().unwrap_or_else(|| "here".to_owned())
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
    /// `text` couldn't be typed into run `n` of `stage`: `why`.
    SendFailed {
        stage: Stage,
        n: u32,
        send: SendKind,
        text: String,
        via: Via,
        why: String,
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
    T3Thread,
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
    /// A write or read worked again: the problem said goes.
    Worked,
    /// `what` failed: `why`, said as a problem.
    Failed { what: String, why: String },
    /// The index took in `lines` new lines.
    Indexed { lines: usize },
    /// Keeping earlier AI work: `done` of `total` features.
    Backfilling { done: usize, total: usize },
    /// Earlier AI work kept for `features` features; `gone` of their chats
    /// were gone already.
    Backfilled { features: usize, gone: usize },
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
        }
    }

    /// Keep AI work under data folder `data`, the thread telling `done`
    /// what it came to.
    pub(crate) fn start(data: &Path, done: channel::Sender<Done>) -> Self {
        // The thread comes later; until then nothing runs.
        let _ = done;
        Self {
            root: Some(root(data)),
            jobs: None,
            seen: HashMap::new(),
            machine: machine(),
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

impl Gerdr {
    /// What the AI work's thread came to.
    pub(crate) fn ai_work_done(&mut self, report: Report) {
        let _ = report;
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
