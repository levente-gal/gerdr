//! Keeping the AI work of features from before it was kept: what their
//! records, prompts, stage files and logs still hold, and the chats still
//! on disk. Once per feature (`cursor.json`'s `backfilled`).
//!
//! Every line it writes has `backfill: true`, timed by what the record or
//! the file says where it says anything. A line already in the feature's
//! events, whoever wrote it, isn't written again: a backfill stopped half
//! way goes on where it was, and what this gerdr told since it started
//! isn't told twice.
//!
//! What `prompts/` holds becomes:
//! - `{stage}-{n}.md`: the prompt of run `n` (`RunStarted`, at the file's
//!   last change when the record no longer has the run), and its
//!   `.session` the id of its chat, looked for as Claude's, Cursor's and
//!   Codex's in turn when the record doesn't say whose;
//! - `{stage}-{n}-back-{k}.md`: `Sent` as `SendBack`, or `PlanReviewBack`
//!   for Plan review;
//! - `fix-plan-{r}-back-{k}.md`: `Sent` to Build as `SendBack`, as the
//!   notes on a fix plan go;
//! - `build-{n}-{what}-{k}.md`: `Sent` to Build as `FixPlan`, the line that
//!   names a fix round's input;
//! - anything else (`fix-plan-{r}.md`, `fix-plan-{r}-comments.json`…):
//!   `Output`.
//!
//! The text of a backfilled `Sent` is the path of its file: the line typed
//! wasn't kept, and the file is in `files`. It went `Via::Pane`, as nothing
//! says otherwise.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::UNIX_EPOCH,
};

use serde::Serialize;
use serde_json::{Value, json};

use super::{
    AgentKind, BlobRef, Chat, Event, FORMAT, Line, SendKind, StartHow, Via, blobs,
    locate::{self, Found, Homes},
    now_ms, observe, thread, write,
};
use crate::feature::{Feature, Stage, agents, artifacts, plan_review, setup, store};

/// What keeping a feature's earlier AI work came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backfilled {
    /// Chats kept.
    pub chats: usize,
    /// Chats already gone.
    pub gone: usize,
}

/// The files of a feature's data folder that aren't stage files: the
/// record and what is kept beside it go in the snapshot, `setup.log` in
/// its parts, and the graph's log is no agent's work.
const NOT_OUTPUTS: [&str; 5] = [
    store::FILE,
    store::HISTORY,
    store::ROUNDS,
    setup::LOG,
    "graph.log",
];

/// The line each setup run starts its part of `setup.log` with.
const SETUP_START: &[u8] = b"== Setup started";

/// The reason of the `Output`s it writes.
const REASON: &str = "backfill";

/// Keep the earlier AI work of the feature in folder `feature` (its record
/// and stage files) into `dir`, its AI work folder, unless `stop`.
pub fn feature(feature: &Path, dir: &Path, stop: &AtomicBool) -> Result<Backfilled, String> {
    feature_with(
        feature,
        dir,
        Homes::here().as_ref(),
        &super::machine(),
        stop,
    )
}

/// `feature` of data folder `data`, the chats looked for among `homes`
/// (none: this machine has no home), the lines written as machine
/// `machine`'s. Stopped, it is `Ok` with what was done so far, and isn't
/// marked done.
pub fn feature_with(
    data: &Path,
    dir: &Path,
    homes: Option<&Homes>,
    machine: &str,
    stop: &AtomicBool,
) -> Result<Backfilled, String> {
    let marks = write::read_marks(dir).map_err(|err| at(&write::marks_path(dir), &err))?;
    let mut done = Backfilled::default();
    let stopped = || stop.load(Ordering::Relaxed);
    if marks.backfilled || stopped() {
        return Ok(done);
    }
    let record = store::read_at(data)?;
    let mut out = Out {
        dir,
        machine,
        told: told(dir)?,
        kept: marks.outputs,
        outputs: BTreeMap::new(),
    };
    let prompts = prompt_files(data)?;

    notes(&mut out, &record)?;
    let runs = runs(&record, data, &prompts);
    let mut chats = HashSet::new();
    let mut gone = HashSet::new();
    for run in &runs {
        if stopped() {
            return Ok(done);
        }
        let (agent, said) = transcript(dir, homes, machine, &record, run, &mut chats, &mut gone)?;
        done.chats = chats.len();
        done.gone = gone.len();
        let value = match run.run {
            Some(_) => run.value.clone(),
            None => json!({
                "n": run.n,
                "started": run.started,
                "agent": agent.map_or("", AgentKind::name),
                "prompt": run.prompt.as_ref().map(|p| p.to_string_lossy()),
            }),
        };
        let prompt = match &run.prompt {
            Some(path) => put_file(dir, path)?,
            None => None,
        };
        let started = Event::RunStarted {
            stage: run.stage,
            n: run.n,
            run: value,
            prompt,
            how: StartHow::Fresh,
        };
        out.say(run.started * 1000, started)?;
        if let Some(said) = said {
            out.say(now_ms(), said)?;
        }
    }
    let steps: [&Step; 3] = [
        &|out| sends(out, &prompts, &runs),
        &|out| from_record(out, &record, data, &prompts),
        &|out| outputs(out, data, &prompts),
    ];
    for step in steps {
        if stopped() {
            return Ok(done);
        }
        step(&mut out)?;
    }
    if stopped() {
        return Ok(done);
    }
    let bytes = serde_json::to_vec_pretty(&record).map_err(|err| err.to_string())?;
    let snapshot = blobs::put(dir, &bytes).map_err(|err| at(dir, &err))?;
    let event = Event::Snapshot {
        record: snapshot.clone(),
    };
    out.say(now_ms(), event)?;
    write::record(dir, &bytes).map_err(|err| at(&write::record_path(dir), &err))?;

    // Read again: the thread's own `Outputs` may have kept some since.
    let mut marks = write::read_marks(dir).map_err(|err| at(&write::marks_path(dir), &err))?;
    marks.backfilled = true;
    marks.history_noted = marks.history_noted.max(record.history.len());
    marks.last_snapshot = Some(snapshot);
    marks.outputs.extend(out.outputs);
    write::write_marks(dir, &marks).map_err(|err| at(&write::marks_path(dir), &err))?;
    Ok(done)
}

/// One step of a backfill, between which it may stop.
type Step<'a> = dyn Fn(&mut Out) -> Result<(), String> + 'a;

/// `err` of the file at `path`, as said.
fn at(path: &Path, err: &io::Error) -> String {
    format!("{}: {err}", path.display())
}

/// When the file `meta` is of last changed, in seconds since the epoch.
fn mtime(meta: &std::fs::Metadata) -> u64 {
    (meta.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// When the file at `path` last changed, in milliseconds; `None` when it
/// isn't there.
fn changed_ms(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| mtime(&m) * 1000)
}

/// A piece of the record as JSON. Its types always serialize.
fn value(of: &impl Serialize) -> Value {
    serde_json::to_value(of).unwrap_or(Value::Null)
}

/// Keep the file at `path` in feature folder `dir`; `None` when it isn't
/// there.
fn put_file(dir: &Path, path: &Path) -> Result<Option<BlobRef>, String> {
    match std::fs::read(path) {
        Ok(bytes) => (blobs::put(dir, &bytes).map(Some)).map_err(|err| at(dir, &err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(at(path, &err)),
    }
}

/// Keep `json` in feature folder `dir`.
fn put_json(dir: &Path, json: &Value) -> Result<BlobRef, String> {
    let bytes = serde_json::to_vec_pretty(json).map_err(|err| err.to_string())?;
    blobs::put(dir, &bytes).map_err(|err| at(dir, &err))
}

/// Where the lines go, and what is there already.
struct Out<'a> {
    dir: &'a Path,
    machine: &'a str,
    /// The keys (see `keys`) of the lines in the feature's events.
    told: HashSet<String>,
    /// The stage files kept already, by name: the SHA-256 of each.
    kept: BTreeMap<String, String>,
    /// The stage files as they are now, by name: the SHA-256 of each, for
    /// `cursor.json`.
    outputs: BTreeMap<String, String>,
}

impl Out<'_> {
    /// Write `event`, at `at_ms`, unless the events have it already.
    fn say(&mut self, at_ms: u64, event: Event) -> Result<(), String> {
        let keys = keys(&event);
        if keys.iter().any(|key| self.told.contains(key)) {
            return Ok(());
        }
        let line = Line {
            v: FORMAT,
            at_ms,
            machine: self.machine.to_owned(),
            backfill: true,
            event,
        };
        write::append(self.dir, &line)
            .map_err(|err| at(&write::events_path(self.dir, self.machine), &err))?;
        self.told.extend(keys);
        Ok(())
    }

    /// Keep file `name` of the data folder, at `path`, as an `Output`.
    fn output(&mut self, name: &str, path: &Path) -> Result<(), String> {
        let read = std::fs::read(path).and_then(|bytes| Ok((bytes, path.metadata()?)));
        let (bytes, meta) = match read {
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(at(path, &err)),
        };
        let blob = BlobRef::of(&bytes);
        let top = !name.contains('/');
        if top {
            self.outputs.insert(name.to_owned(), blob.sha.clone());
            // The thread kept it as it is now already.
            if self.kept.get(name) == Some(&blob.sha) {
                return Ok(());
            }
        }
        blobs::put(self.dir, &bytes).map_err(|err| at(self.dir, &err))?;
        let event = Event::Output {
            file: name.to_owned(),
            blob,
            reason: REASON.to_owned(),
        };
        self.say(mtime(&meta) * 1000, event)
    }
}

/// What tells a line apart from the others of its kind: one whose key is
/// in the events already isn't written again. None: nothing does.
fn keys(event: &Event) -> Vec<String> {
    let one = match event {
        Event::Note { i, .. } => format!("note {i}"),
        Event::RunStarted { stage, n, .. } => format!("run {stage:?} {n}"),
        Event::Marked { stage, mark } => format!("marked {stage:?} {}", mark["at"]),
        Event::Status { to, .. } => format!("status {to}"),
        Event::FixRoundOpened { n, .. } => format!("fix round opened {n}"),
        Event::FixRoundClosed { n, .. } => format!("fix round closed {n}"),
        Event::ReviewRead { n, .. } => format!("review {n}"),
        Event::PlanReviewRead { n, .. } => format!("plan review {n}"),
        Event::VerifyStarted { run, .. } => format!("verify {run}"),
        Event::CheckEnded { run, i, .. } => format!("check {run} {i}"),
        Event::VerifyEnded { run, .. } => format!("verified {run}"),
        Event::SetupEnded { run, .. } => format!("setup {} {}", run["machine"], run["started"]),
        Event::Manual { .. } => "manual".to_owned(),
        Event::Snapshot { record } => format!("snapshot {}", record.sha),
        Event::Output { file, blob, .. } => format!("output {file} {}", blob.sha),
        Event::TranscriptMissing { stage, n, .. } => format!("missing {stage:?} {n}"),
        // A line typed names its files: each tells it.
        Event::Sent { files, .. } => return files.keys().map(|f| format!("sent {f}")).collect(),
        _ => return Vec::new(),
    };
    vec![one]
}

/// The keys of the lines in the events of feature folder `dir`, every
/// machine's.
fn told(dir: &Path) -> Result<HashSet<String>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(err) => return Err(at(dir, &err)),
    };
    let mut told = HashSet::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("events-") && name.ends_with(".jsonl")) {
            continue;
        }
        let path = entry.path();
        let text = std::fs::read_to_string(&path).map_err(|err| at(&path, &err))?;
        // A line cut short, or of a later format, tells nothing.
        let lines = (text.lines()).filter_map(|l| serde_json::from_str::<Line>(l).ok());
        told.extend(lines.flat_map(|line| keys(&line.event)));
    }
    Ok(told)
}

/// The history: a `Note` a line.
fn notes(out: &mut Out, record: &Feature) -> Result<(), String> {
    for (i, event) in record.history.iter().enumerate() {
        let note = Event::Note {
            i,
            at: event.at,
            what: event.what.clone(),
        };
        out.say(event.at * 1000, note)?;
    }
    Ok(())
}

/// What a file of `prompts/` is, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prompt {
    /// `{stage}-{n}.md`: run `n`'s prompt.
    Start(Stage, u32),
    /// `{stage}-{n}.session`: run `n`'s chat id.
    Session(Stage, u32),
    /// `{stage}-{n}-back-{k}.md`: comments sent back to run `n`.
    Back(Stage, u32),
    /// `fix-plan-{r}-back-{k}.md`: notes on fix round `r`'s plan.
    FixPlanBack,
    /// `build-{n}-{what}-{k}.md`: a fix round's input, for Build run `n`.
    FixInput(u32),
    /// Anything else: kept as it is.
    Other,
}

/// The file of `prompts/` named `name`.
fn prompt_kind(name: &str) -> Prompt {
    if let Some(rest) = name.strip_prefix("fix-plan-") {
        let back = (rest.strip_suffix(".md"))
            .and_then(|r| r.split_once("-back-"))
            .is_some_and(|(r, k)| number(r).is_some() && number(k).is_some());
        return if back {
            Prompt::FixPlanBack
        } else {
            Prompt::Other
        };
    }
    let (stem, session) = match (name.strip_suffix(".md"), name.strip_suffix(".session")) {
        (Some(stem), _) => (stem, false),
        (_, Some(stem)) => (stem, true),
        _ => return Prompt::Other,
    };
    // The longest first: `plan-review-1` isn't Plan's.
    let mut stages: Vec<(Stage, String)> = (Stage::ALL.into_iter())
        .map(|stage| (stage, stage_key(stage)))
        .collect();
    stages.sort_by_key(|(_, key)| std::cmp::Reverse(key.len()));
    for (stage, key) in stages {
        let Some(rest) = (stem.strip_prefix(key.as_str())).and_then(|r| r.strip_prefix('-')) else {
            continue;
        };
        let digits = (rest.find(|c: char| !c.is_ascii_digit())).unwrap_or(rest.len());
        let Some(n) = number(&rest[..digits]) else {
            continue;
        };
        return match (&rest[digits..], session) {
            ("", false) => Prompt::Start(stage, n),
            ("", true) => Prompt::Session(stage, n),
            (_, true) => Prompt::Other,
            (rest, false) if rest.strip_prefix("-back-").and_then(number).is_some() => {
                Prompt::Back(stage, n)
            }
            (rest, false) if stage == Stage::Build => {
                let input = (rest.strip_prefix('-'))
                    .and_then(|r| r.rsplit_once('-'))
                    .is_some_and(|(what, k)| !what.is_empty() && number(k).is_some());
                match input {
                    true => Prompt::FixInput(n),
                    false => Prompt::Other,
                }
            }
            _ => Prompt::Other,
        };
    }
    Prompt::Other
}

/// `text` as a number, when it is digits only.
fn number(text: &str) -> Option<u32> {
    match !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        true => text.parse().ok(),
        false => None,
    }
}

/// `stage` as the names of its files have it: `ai-review`.
fn stage_key(stage: Stage) -> String {
    value(&stage).as_str().unwrap_or_default().to_owned()
}

/// A file of `prompts/`.
struct PromptFile {
    name: String,
    path: PathBuf,
    /// When it last changed, in seconds since the epoch.
    mtime: u64,
    kind: Prompt,
}

/// The files of `prompts/` in data folder `data`, by name.
fn prompt_files(data: &Path) -> Result<Vec<PromptFile>, String> {
    let folder = data.join("prompts");
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(at(&folder, &err)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| at(&folder, &err))?;
        let meta = entry.metadata().map_err(|err| at(&entry.path(), &err))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !meta.is_file() || name.starts_with('.') {
            continue;
        }
        files.push(PromptFile {
            kind: prompt_kind(&name),
            path: entry.path(),
            mtime: mtime(&meta),
            name,
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// A run of a stage, from the record, its prompt or both.
struct Run<'a> {
    stage: Stage,
    n: u32,
    /// The record's, while it still has it.
    run: Option<&'a agents::StageRun>,
    /// The record's run as JSON.
    value: Value,
    /// Its prompt in `prompts/`, if there.
    prompt: Option<PathBuf>,
    /// Its chat's id, from its `.session`.
    session: Option<String>,
    /// When it began, in seconds since the epoch: the record's, else its
    /// prompt's last change.
    started: u64,
}

/// The id in the `.session` file at `path`, if any.
fn session_at(path: &Path) -> Option<String> {
    let id = std::fs::read_to_string(path).ok()?;
    Some(id.trim().to_owned()).filter(|id| !id.is_empty())
}

/// The runs of `record` and those `prompts` of data folder `data` tell of,
/// by stage and number.
fn runs<'a>(record: &'a Feature, data: &Path, prompts: &[PromptFile]) -> Vec<Run<'a>> {
    let folder = data.join("prompts");
    let mut runs: BTreeMap<(Stage, u32), Run> = BTreeMap::new();
    for (stage, run) in observe::runs_of(record) {
        // Its prompt by name here: the record's path is where it was made.
        let prompt = (run.prompt.file_name()).map(|name| folder.join(name));
        let session = (prompt.as_deref()).and_then(|p| session_at(&agents::session_file(p)));
        let run = Run {
            stage,
            n: run.n,
            run: Some(run),
            value: value(run),
            prompt: prompt.filter(|p| p.is_file()),
            session,
            started: run.started,
        };
        runs.insert((stage, run.n), run);
    }
    for file in prompts {
        let (Prompt::Start(stage, n) | Prompt::Session(stage, n)) = file.kind else {
            continue;
        };
        let run = runs.entry((stage, n)).or_insert_with(|| Run {
            stage,
            n,
            run: None,
            value: Value::Null,
            prompt: None,
            session: None,
            started: file.mtime,
        });
        match file.kind {
            Prompt::Start(..) => {
                if run.prompt.is_none() {
                    run.prompt = Some(file.path.clone());
                }
                if run.run.is_none() {
                    run.started = file.mtime;
                }
            }
            _ if run.session.is_none() => run.session = session_at(&file.path),
            _ => {}
        }
    }
    runs.into_values().collect()
}

/// The chat of `run` of `record`.
fn chat_of(record: &Feature, run: &Run) -> Chat {
    if let Some(r) = run.run {
        return observe::chat(record, run.stage, r, run.session.clone());
    }
    let mut folders = vec![record.dir.clone()];
    for worktree in record.repos.iter().map(|r| &r.worktree) {
        if !folders.contains(worktree) {
            folders.push(worktree.clone());
        }
    }
    Chat {
        agent: AgentKind::Unknown,
        id: run.session.clone(),
        stage: run.stage,
        n: run.n,
        started: run.started,
        folders,
        machine: None,
    }
}

/// `chat` among `homes`; `dir` the feature folder that may keep it. With
/// no agent known, its id is looked for as Claude's, Cursor's, then
/// Codex's: the names of the files don't say whose it is.
fn look(homes: &Homes, dir: &Path, chat: &Chat) -> Result<Option<Found>, String> {
    let kinds = match chat.agent {
        AgentKind::Unknown => vec![AgentKind::Claude, AgentKind::Cursor, AgentKind::Codex],
        kind => vec![kind],
    };
    for agent in kinds {
        let chat = Chat {
            agent,
            ..chat.clone()
        };
        let chat = thread::with_known_id(dir, &chat);
        if let Some(found) = locate::find(homes, &chat, &thread::claimed(dir, &chat))? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Keep the chat of `run` of `record` in feature folder `dir`, if it is
/// still among `homes`: the agent it was found of, and the line to write,
/// that it was kept or that it is missing. `chats` gathers the chats
/// found, `gone` those already gone.
fn transcript(
    dir: &Path,
    homes: Option<&Homes>,
    machine: &str,
    record: &Feature,
    run: &Run,
    chats: &mut HashSet<(AgentKind, String)>,
    gone: &mut HashSet<String>,
) -> Result<(Option<AgentKind>, Option<Event>), String> {
    let chat = chat_of(record, run);
    let missing = |why: &str| {
        Some(Event::TranscriptMissing {
            agent: chat.agent,
            session: chat.id.clone(),
            stage: chat.stage,
            n: chat.n,
            why: why.to_owned(),
        })
    };
    // Codex's and OpenCode's are found by folder and time.
    let by_time = matches!(chat.agent, AgentKind::Codex | AgentKind::OpenCode);
    if chat.id.is_none() && !by_time {
        return Ok((None, missing("its chat id wasn't kept")));
    }
    let Some(homes) = homes else {
        return Ok((None, missing("this machine has no home folder (HOME)")));
    };
    let found = match look(homes, dir, &chat) {
        Ok(found) => found,
        Err(why) => {
            tracing::warn!(
                "ai-work: looking for the chat of {:?} {}: {why}",
                chat.stage,
                chat.n
            );
            return Ok((None, missing(&format!("couldn't be looked for: {why}"))));
        }
    };
    let Some(found) = found else {
        let id = (chat.id.clone()).unwrap_or_else(|| format!("{:?} {}", chat.stage, chat.n));
        gone.insert(id);
        let why = match chat.machine {
            Some(_) => "already gone, or on its machine",
            None => "already gone",
        };
        return Ok((None, missing(why)));
    };
    let ran_on = chat.machine.as_deref().unwrap_or(machine);
    // Kept already as it is, it was said then.
    let kept = thread::keep(dir, &found, (chat.stage, chat.n), ran_on, None)
        .map_err(|err| at(&found.main, &err))?;
    chats.insert((found.agent, found.id.clone()));
    Ok((Some(found.agent), kept))
}

/// The lines typed into the agents whose files `prompts` holds; `runs` the
/// feature's.
fn sends(out: &mut Out, prompts: &[PromptFile], runs: &[Run]) -> Result<(), String> {
    // The Build run at work when a fix plan's notes went: the last begun.
    let build_at = |secs: u64| {
        let builds = runs.iter().filter(|r| r.stage == Stage::Build);
        let before = (builds.clone().filter(|r| r.started <= secs))
            .map(|r| r.n)
            .max();
        before.or_else(|| builds.map(|r| r.n).max()).unwrap_or(0)
    };
    for file in prompts {
        let (stage, n, send) = match file.kind {
            Prompt::Back(Stage::PlanReview, n) => (Stage::PlanReview, n, SendKind::PlanReviewBack),
            Prompt::Back(stage, n) => (stage, n, SendKind::SendBack),
            Prompt::FixPlanBack => (Stage::Build, build_at(file.mtime), SendKind::SendBack),
            Prompt::FixInput(n) => (Stage::Build, n, SendKind::FixPlan),
            _ => continue,
        };
        let Some(blob) = put_file(out.dir, &file.path)? else {
            continue;
        };
        let path = file.path.to_string_lossy().into_owned();
        let event = Event::Sent {
            stage,
            n,
            send,
            text: path.clone(),
            files: BTreeMap::from([(path, blob)]),
            via: Via::Pane,
        };
        out.say(file.mtime * 1000, event)?;
    }
    Ok(())
}

/// What `record` holds beside its history and runs: marks, reviews,
/// Verify, setups, fix rounds, the manual test sheet and its status. Its
/// data folder is `data`, its `prompts/` files `prompts`.
fn from_record(
    out: &mut Out,
    record: &Feature,
    data: &Path,
    prompts: &[PromptFile],
) -> Result<(), String> {
    let now = now_ms();
    for (&stage, mark) in &record.marks {
        let event = Event::Marked {
            stage,
            mark: value(mark),
        };
        out.say(mark.at * 1000, event)?;
    }

    // The AI reviews: each earlier round, then the latest. Each is said
    // read about when its run began, as the record doesn't time them.
    let began = |n: u32| {
        (prompts.iter())
            .find(|f| f.kind == Prompt::Start(Stage::AiReview, n))
            .map(|f| f.mtime * 1000)
    };
    if let Some(review) = record.review.as_ref().filter(|r| r.n > 0) {
        let latest = json!({ "n": review.n, "findings": review.findings });
        let rounds = (review.rounds.iter())
            .filter(|round| round.n != review.n)
            .map(|round| (round.n, round.findings.len(), value(round)))
            .chain([(review.n, review.findings.len(), latest)]);
        for (n, findings, round) in rounds {
            let event = Event::ReviewRead {
                n,
                findings,
                review: Some(put_json(out.dir, &round)?),
            };
            out.say(began(n).unwrap_or(now), event)?;
        }
    }
    if let Some(review) = record.plan_review.as_ref().filter(|r| r.n > 0) {
        let event = Event::PlanReviewRead {
            n: review.n,
            suggestions: review.suggestions.len(),
            review: Some(put_json(out.dir, &value(&review.suggestions))?),
        };
        let file = data.join(plan_review::FILE);
        out.say(changed_ms(&file).unwrap_or(now), event)?;
    }

    // Verify: what its last run kept. What the checks printed wasn't.
    if let Some(verify) = &record.verify {
        let run = observe::verify_key(verify);
        let event = Event::VerifyStarted {
            run: run.clone(),
            verify: value(verify),
        };
        out.say(verify.started * 1000, event)?;
        let ended = verify.finished.unwrap_or(verify.started) * 1000;
        for (i, check) in verify.checks.iter().enumerate() {
            let Some(outcome) = check.outcome.as_ref().filter(|_| check.run == verify.n) else {
                continue;
            };
            let event = Event::CheckEnded {
                run: run.clone(),
                key: observe::check_key(&check.repo, &check.command),
                i,
                outcome: value(outcome),
                log: None,
                truncated: false,
            };
            out.say(ended, event)?;
        }
        if let Some(finished) = verify.finished {
            let event = Event::VerifyEnded {
                run,
                verify: value(verify),
            };
            out.say(finished * 1000, event)?;
        }
    }

    // The setups. Each part of `setup.log` is kept; the last one is the
    // last setup's here.
    let log = setup::log_path(data);
    let raw = match std::fs::read(&log) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(at(&log, &err)),
    };
    let log_at = changed_ms(&log).unwrap_or(now);
    let mut last = None;
    for part in setup_parts(&raw) {
        let blob = blobs::put(out.dir, part).map_err(|err| at(out.dir, &err))?;
        let event = Event::Output {
            file: setup::LOG.to_owned(),
            blob: blob.clone(),
            reason: REASON.to_owned(),
        };
        out.say(log_at, event)?;
        last = Some(blob);
    }
    let setups = (record.setup.iter().map(|s| (s, true)))
        .chain(record.setups_elsewhere.iter().map(|s| (s, false)));
    for (run, here) in setups {
        let Some(finished) = run.finished else {
            continue;
        };
        let event = Event::SetupEnded {
            run: value(run),
            log: last.clone().filter(|_| here),
        };
        out.say(finished * 1000, event)?;
    }

    // The fix rounds it still has: the last one over, and the one open.
    for round in (record.last_fix_round.iter()).chain(&record.fix_round) {
        let event = Event::FixRoundOpened {
            n: round.n,
            round: value(round),
        };
        out.say(round.asked * 1000, event)?;
        if let Some(ended) = round.ended {
            let event = Event::FixRoundClosed {
                n: round.n,
                round: value(round),
            };
            out.say(ended * 1000, event)?;
        }
    }

    if let Some(manual) = &record.manual {
        let event = Event::Manual {
            manual: value(manual),
        };
        out.say(now, event)?;
    }
    let event = Event::Status {
        from: Value::Null,
        to: value(&record.status),
    };
    out.say(now, event)
}

/// `setup.log` text `raw` in the parts each setup run wrote: from one
/// `== Setup started` at the start of a line to the next. What comes
/// before the first (an older gerdr's log has none) is a part too.
fn setup_parts(raw: &[u8]) -> Vec<&[u8]> {
    let mut starts: Vec<usize> = (0..raw.len())
        .filter(|&at| (at == 0 || raw[at - 1] == b'\n') && raw[at..].starts_with(SETUP_START))
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    let ends = (starts.iter().skip(1).copied()).chain([raw.len()]);
    (starts.iter().zip(ends))
        .map(|(&from, to)| &raw[from..to])
        .filter(|part| !part.is_empty())
        .collect()
}

/// The stage files of data folder `data`, its `archive/`, and what of
/// `prompts` is no run's prompt, chat id or line typed.
fn outputs(out: &mut Out, data: &Path, prompts: &[PromptFile]) -> Result<(), String> {
    let entries = std::fs::read_dir(data).map_err(|err| at(data, &err))?;
    let mut names = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|err| at(data, &err))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let plain = entry.file_type().is_ok_and(|kind| kind.is_file());
        let other = name.starts_with('.')
            || name.ends_with(".tmp")
            || name.ends_with(".lock")
            || name.contains(".bad-")
            || NOT_OUTPUTS.contains(&name.as_str());
        if plain && !other {
            names.insert(name);
        }
    }
    for name in names {
        out.output(&name, &data.join(&name))?;
    }
    for (rel, path) in files_under(&data.join(artifacts::ARCHIVE)) {
        out.output(&format!("{}/{rel}", artifacts::ARCHIVE), &path)?;
    }
    for file in prompts.iter().filter(|f| f.kind == Prompt::Other) {
        out.output(&format!("prompts/{}", file.name), &file.path)?;
    }
    Ok(())
}

/// The files under folder `base`, by their path from it, in order. Links
/// aren't followed.
fn files_under(base: &Path) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return,
            Err(err) => {
                // The rest is kept; this folder is said in the log.
                tracing::warn!("ai-work: reading {}: {err}", dir.display());
                return;
            }
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path(), out),
                Ok(kind) if kind.is_file() => out.push(entry.path()),
                _ => {}
            }
        }
    }
    let mut paths = Vec::new();
    walk(base, &mut paths);
    let mut files: Vec<(String, PathBuf)> = (paths.into_iter())
        .filter_map(|path| {
            let rel = path.strip_prefix(base).ok()?.to_str()?.to_owned();
            Some((rel, path))
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::feature::{
        Mark,
        agents::StageRun,
        review_import::{AiReview, Finding, ReviewRound},
        setup::SetupRun,
        tests::feature as record,
        verify::{CheckRun, Outcome, VerifyRun},
    };

    /// A feature's data folder, its AI work folder and a home, under a
    /// folder of its own for test `name`.
    struct Fixture {
        data: PathBuf,
        dir: PathBuf,
        homes: Homes,
    }

    fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A feature shaped like a real one: Plan's chat still in Claude's
    /// store, Build's gone, and an older Discover run the record forgot.
    fn fixture(name: &str) -> Fixture {
        let root = super::super::write::tests::scratch(name);
        let data = root.join("features/p/s");
        let dir = root.join("ai-work/p/s");
        let home = root.join("home");
        let prompts = data.join("prompts");
        let run = |n: u32, started: u64, file: &str| StageRun {
            n,
            agent: "Claude Code".into(),
            base: "claude".into(),
            started,
            // Where it was made: the prompt is read by its name here.
            prompt: PathBuf::from("/elsewhere/prompts").join(file),
            ..StageRun::default()
        };
        let mut f = record("s", 100);
        f.history = vec![
            crate::feature::Event {
                at: 200,
                what: "Created".into(),
            },
            crate::feature::Event {
                at: 300,
                what: "Plan ticked".into(),
            },
        ];
        f.runs.insert(Stage::Plan, run(1, 1000, "plan-1.md"));
        f.runs.insert(Stage::Build, run(2, 2000, "build-2.md"));
        f.marks.insert(
            Stage::Plan,
            Mark {
                at: 1500,
                by: "you".into(),
                note: String::new(),
                patch: None,
            },
        );
        f.review = Some(AiReview {
            n: 2,
            findings: vec![Finding::default()],
            rounds: vec![ReviewRound {
                n: 1,
                findings: vec![Finding::default(), Finding::default()],
            }],
            ..AiReview::default()
        });
        f.verify = Some(VerifyRun {
            n: 1,
            started: 3000,
            finished: Some(3100),
            checks: vec![CheckRun {
                repo: "api".into(),
                command: "npm test".into(),
                run: 1,
                outcome: Some(Outcome::default()),
                ..CheckRun::default()
            }],
            ..VerifyRun::default()
        });
        f.setup = Some(SetupRun {
            started: 900,
            finished: Some(950),
            ..SetupRun::default()
        });
        std::fs::create_dir_all(&data).unwrap();
        store::save_at(&data, &f).unwrap();
        for (file, text) in [
            ("plan-1.md", "plan prompt"),
            ("plan-1.session", "aaaa-1111\n"),
            ("build-2.md", "build prompt"),
            ("build-2.session", "bbbb-2222"),
            ("discover-1.md", "old discover prompt"),
            ("build-2-back-1.md", "comments"),
            ("build-2-findings-1.md", "2 findings"),
            ("fix-plan-1.md", "old fix plan"),
            ("fix-plan-1-back-1.md", "notes on the plan"),
        ] {
            put(&prompts.join(file), text);
        }
        for (file, text) in [
            ("brief.md", "the brief"),
            ("plan.md", "the plan"),
            ("plan-written.md", "the plan"),
            ("graph.log", "graphify ran"),
            ("archive/brief-1.md", "an older brief"),
            (
                setup::LOG,
                "== Setup started 1\nfirst\n== Setup started 2\nsecond\n",
            ),
        ] {
            put(&data.join(file), text);
        }
        put(
            &home.join(".claude/projects/-r/aaaa-1111.jsonl"),
            "{\"type\":\"user\"}\n",
        );
        Fixture {
            data,
            dir,
            homes: Homes::under(&home),
        }
    }

    fn run(fx: &Fixture, stop: bool) -> Backfilled {
        let stop = AtomicBool::new(stop);
        feature_with(&fx.data, &fx.dir, Some(&fx.homes), "m1", &stop).unwrap()
    }

    fn lines(fx: &Fixture) -> Vec<Line> {
        let path = write::events_path(&fx.dir, "m1");
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn kind(event: &Event) -> String {
        value(event)["kind"].as_str().unwrap().to_owned()
    }

    fn count(lines: &[Line], name: &str) -> usize {
        lines.iter().filter(|l| kind(&l.event) == name).count()
    }

    #[test]
    fn a_feature_s_earlier_work_is_kept() {
        let fx = fixture("backfill");
        assert_eq!(run(&fx, false), Backfilled { chats: 1, gone: 1 });
        let lines = lines(&fx);
        assert!(lines.iter().all(|l| l.backfill && l.machine == "m1"));
        assert_eq!(count(&lines, "note"), 2);
        assert_eq!(count(&lines, "run_started"), 3);
        assert_eq!(count(&lines, "transcript"), 1);
        assert_eq!(count(&lines, "transcript_missing"), 2);
        assert_eq!(count(&lines, "sent"), 3);
        assert_eq!(count(&lines, "review_read"), 2);
        assert_eq!(count(&lines, "verify_started"), 1);
        assert_eq!(count(&lines, "check_ended"), 1);
        assert_eq!(count(&lines, "verify_ended"), 1);
        assert_eq!(count(&lines, "setup_ended"), 1);
        assert_eq!(count(&lines, "marked"), 1);
        assert_eq!(count(&lines, "status"), 1);
        assert_eq!(count(&lines, "snapshot"), 1);
        // setup.log's two parts, brief, plan, plan-written, the archive's
        // brief and the old fix plan; not graph.log nor the record.
        let files: Vec<String> = (lines.iter())
            .filter_map(|l| match &l.event {
                Event::Output { file, .. } => Some(file.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            files,
            [
                "setup.log",
                "setup.log",
                "brief.md",
                "plan-written.md",
                "plan.md",
                "archive/brief-1.md",
                "prompts/fix-plan-1.md"
            ]
        );

        // The runs: the record's at its times, the forgotten one at its
        // prompt's, each with its prompt.
        let started: Vec<(Stage, u32, u64, Vec<u8>)> = (lines.iter())
            .filter_map(|l| match &l.event {
                Event::RunStarted {
                    stage, n, prompt, ..
                } => Some((
                    *stage,
                    *n,
                    l.at_ms,
                    blobs::get(&fx.dir, prompt.as_ref()?).unwrap(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            started[1..].to_vec(),
            [
                (Stage::Plan, 1, 1_000_000, b"plan prompt".to_vec()),
                (Stage::Build, 2, 2_000_000, b"build prompt".to_vec()),
            ]
        );
        assert_eq!(started[0].0, Stage::Discover);
        assert_eq!(started[0].3, b"old discover prompt");

        // Plan's chat is kept; Build's is gone.
        let session = thread::session_dir(&fx.dir, AgentKind::Claude, "aaaa-1111");
        assert!(session.join(thread::TRANSCRIPT).is_file());
        let gone = (lines.iter()).find_map(|l| match &l.event {
            Event::TranscriptMissing {
                stage: Stage::Build,
                session,
                why,
                ..
            } => Some((session.clone(), why.clone())),
            _ => None,
        });
        assert_eq!(
            gone,
            Some((Some("bbbb-2222".into()), "already gone".into()))
        );

        // What was sent, as what.
        let sent: Vec<(Stage, u32, SendKind)> = (lines.iter())
            .filter_map(|l| match &l.event {
                Event::Sent { stage, n, send, .. } => Some((*stage, *n, *send)),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            [
                (Stage::Build, 2, SendKind::SendBack),
                (Stage::Build, 2, SendKind::FixPlan),
                (Stage::Build, 2, SendKind::SendBack),
            ]
        );

        // The last setup's log is its part.
        let log = (lines.iter()).find_map(|l| match &l.event {
            Event::SetupEnded { log, .. } => log.clone(),
            _ => None,
        });
        let log = blobs::get(&fx.dir, &log.unwrap()).unwrap();
        assert_eq!(log, b"== Setup started 2\nsecond\n");

        // Done, and marked so.
        let marks = write::read_marks(&fx.dir).unwrap();
        assert!(marks.backfilled);
        assert_eq!(marks.history_noted, 2);
        assert!(marks.last_snapshot.is_some());
        assert!(marks.outputs.contains_key("brief.md"));
        assert!(!marks.outputs.contains_key("archive/brief-1.md"));
        let kept: Feature =
            serde_json::from_slice(&std::fs::read(write::record_path(&fx.dir)).unwrap()).unwrap();
        assert_eq!(kept.history.len(), 2, "the history beside it is in it");
    }

    #[test]
    fn the_same_content_is_kept_once() {
        let fx = fixture("backfill-dedup");
        run(&fx, false);
        let blob = |name: &str| {
            lines(&fx).into_iter().find_map(|l| match l.event {
                Event::Output { file, blob, .. } if file == name => Some(blob),
                _ => None,
            })
        };
        assert_eq!(blob("plan.md"), blob("plan-written.md"));
        let refs: HashSet<String> = (lines(&fx).into_iter())
            .flat_map(|l| {
                let v = value(&l.event);
                let mut shas = Vec::new();
                collect_shas(&v, &mut shas);
                shas
            })
            .collect();
        let kept = files_under(&fx.dir.join("blobs")).len();
        assert_eq!(kept, refs.len(), "one file per content");
    }

    /// The `sha` of every blob `v` names.
    fn collect_shas(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                if let (Some(Value::String(sha)), Some(Value::Number(_))) =
                    (map.get("sha"), map.get("len"))
                {
                    out.push(sha.clone());
                }
                map.values().for_each(|v| collect_shas(v, out));
            }
            Value::Array(items) => items.iter().for_each(|v| collect_shas(v, out)),
            _ => {}
        }
    }

    #[test]
    fn a_second_backfill_writes_nothing() {
        let fx = fixture("backfill-again");
        run(&fx, false);
        let before = std::fs::read(write::events_path(&fx.dir, "m1")).unwrap();
        assert_eq!(run(&fx, false), Backfilled::default());
        let after = std::fs::read(write::events_path(&fx.dir, "m1")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn a_stopped_backfill_goes_on_without_telling_twice() {
        let fx = fixture("backfill-stop");
        assert_eq!(run(&fx, true), Backfilled::default());
        assert!(lines(&fx).is_empty());
        assert!(!write::read_marks(&fx.dir).unwrap().backfilled);

        // What was told since gerdr started isn't told again.
        let noted = Line {
            v: FORMAT,
            at_ms: 300_000,
            machine: "m1".into(),
            backfill: false,
            event: Event::Note {
                i: 1,
                at: 300,
                what: "Plan ticked".into(),
            },
        };
        write::append(&fx.dir, &noted).unwrap();
        run(&fx, false);
        let first = lines(&fx);
        assert_eq!(count(&first, "note"), 2);

        // Stopped after it wrote all but its mark: the next writes nothing.
        let mut marks = write::read_marks(&fx.dir).unwrap();
        marks.backfilled = false;
        write::write_marks(&fx.dir, &marks).unwrap();
        assert_eq!(run(&fx, false), Backfilled { chats: 1, gone: 1 });
        assert_eq!(lines(&fx), first);
    }

    #[test]
    fn prompt_files_are_told_by_name() {
        use Prompt::*;
        for (name, kind) in [
            ("plan-1.md", Start(Stage::Plan, 1)),
            ("plan-review-2.md", Start(Stage::PlanReview, 2)),
            ("ai-review-14.session", Session(Stage::AiReview, 14)),
            ("plan-review-2-back-1.md", Back(Stage::PlanReview, 2)),
            ("build-3-back-2.md", Back(Stage::Build, 3)),
            ("build-1-findings-and-comment-1.md", FixInput(1)),
            ("build-3-verify-failures-2.md", FixInput(3)),
            ("fix-plan-4-back-1.md", FixPlanBack),
            ("fix-plan-4.md", Other),
            ("fix-plan-1-comments.json", Other),
            ("plan-1-findings-1.md", Other),
            ("notes.md", Other),
        ] {
            assert_eq!(prompt_kind(name), kind, "{name}");
        }
    }

    #[test]
    fn setup_log_is_cut_at_each_start() {
        let raw = b"old\n== Setup started 1\na\nsays == Setup started\n== Setup started 2\nb\n";
        assert_eq!(
            setup_parts(raw),
            [
                &b"old\n"[..],
                b"== Setup started 1\na\nsays == Setup started\n",
                b"== Setup started 2\nb\n",
            ]
        );
        assert_eq!(setup_parts(b"no runs\n"), [&b"no runs\n"[..]]);
        assert!(setup_parts(b"").is_empty());
    }

    /// A copy of a real feature's folder, backfilled with this machine's
    /// chats read where they are: `cargo test --bin gerdr dry_run --
    /// --ignored --nocapture`. Nothing is written but under the temp
    /// folder.
    #[test]
    #[ignore]
    fn dry_run_on_a_copy_of_a_real_feature() {
        let real = crate::home_dir()
            .unwrap()
            .join(".local/share/gerdr/features/ao-app/custom-decision-port");
        let root = super::super::write::tests::scratch("backfill-dry-run");
        let data = root.join("features/ao-app/custom-decision-port");
        std::fs::create_dir_all(data.parent().unwrap()).unwrap();
        let copied = std::process::Command::new("cp")
            .arg("-a")
            .arg(&real)
            .arg(&data)
            .status()
            .unwrap();
        assert!(copied.success());
        let dir = root.join("ai-work/ao-app/custom-decision-port");
        let stop = AtomicBool::new(false);
        let began = std::time::Instant::now();
        let done = feature_with(&data, &dir, Homes::here().as_ref(), "here", &stop).unwrap();
        let took = began.elapsed();
        let lines: Vec<Line> = std::fs::read_to_string(write::events_path(&dir, "here"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for line in &lines {
            *kinds.entry(kind(&line.event)).or_default() += 1;
        }
        let size = |path: &Path| -> u64 {
            files_under(path)
                .iter()
                .map(|(_, p)| std::fs::metadata(p).unwrap().len())
                .sum()
        };
        println!("{done:?} in {took:?}");
        println!("{} lines: {kinds:?}", lines.len());
        println!(
            "archive {} bytes (blobs {}, sessions {}); data folder {} bytes",
            size(&dir),
            size(&dir.join("blobs")),
            size(&dir.join("sessions")),
            size(&data)
        );
        for line in &lines {
            if let Event::TranscriptMissing {
                stage,
                n,
                why,
                agent,
                ..
            } = &line.event
            {
                println!("missing {stage:?} {n} ({agent:?}): {why}");
            }
        }
        assert_eq!(
            feature_with(&data, &dir, None, "here", &stop).unwrap(),
            Backfilled::default()
        );
    }
}
