//! The `ai-work` thread: everything too slow for the loop. It copies
//! transcripts, keeps stage files and logs as blobs, and takes new lines
//! into the index, a batch every 2 s in one transaction. What it came to
//! goes back as `Done::AiWork`.
//!
//! A chat is kept in `sessions/<agent>-<id>/` of its feature's folder:
//! `meta.json` says how much of it is kept (see `Meta`), the transcript is
//! `transcript.jsonl.zst` (frames appended as it grows) or, for Cursor,
//! `store.db.zst` (a snapshot taken again when the database moved), and
//! the files that go with it are `extra/<rel>.zst`. A chat on another
//! machine is pulled into `.staging/<host>/` first, on a thread of its own.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use smithay::reexports::calloop::channel;

use super::{
    AgentKind, BlobRef, Chat, Event, FORMAT, Line, Report, blobs,
    locate::{self, Form, Found, Homes},
    now_ms, write,
    zst::{self, FRAME_LEVEL},
};
use crate::{
    feature::{
        Stage,
        remote::{Reach, Side},
    },
    features::Done,
};

/// What the thread is asked to do.
#[derive(Debug)]
pub enum Job {
    /// Take the new lines of feature `slug` into the index.
    Index { project: String, slug: String },
    /// Keep `chat` of feature `slug` as it is now. `there`: how to reach the
    /// machine it ran on; `None` here.
    Capture {
        project: String,
        slug: String,
        chat: Chat,
        there: Option<(Reach, Side)>,
    },
    /// Keep the stage files `files` (name, path) of feature `slug`, those
    /// that changed, `reason`.
    Outputs {
        project: String,
        slug: String,
        files: Vec<(String, PathBuf)>,
        reason: String,
    },
    /// Keep the part of `log` (a feature's `setup.log`) the setup `run`
    /// wrote.
    SetupSlice {
        project: String,
        slug: String,
        run: Value,
        log: PathBuf,
    },
    /// The chats at work now, by project and slug: kept again every 10
    /// minutes while they last.
    Watch { chats: Vec<(String, String, Chat)> },
    /// Draw the index again from the files.
    Rescan,
    /// Keep the earlier AI work of features `features` (project, slug).
    Backfill { features: Vec<(String, String)> },
    /// `chat` of feature `slug` was pulled from another machine into
    /// `staging`.
    Pulled {
        project: String,
        slug: String,
        chat: Chat,
        staging: PathBuf,
    },
    /// End.
    Stop,
}

/// New lines wait this long for others, to go into the index together.
const BATCH: Duration = Duration::from_secs(2);
/// The chats at work are kept again this often.
const SWEEP: Duration = Duration::from_secs(10 * 60);
/// The longest the thread waits for a job before looking at the time.
const TICK: Duration = Duration::from_millis(500);
/// A chat's transcript in its session folder.
pub const TRANSCRIPT: &str = "transcript.jsonl.zst";
/// A Cursor chat's database in its session folder.
pub const STORE: &str = "store.db.zst";
/// A chat's `meta.json`.
pub const META: &str = "meta.json";
/// Where a pulled chat says where it came from, in its staging folder.
const SOURCE: &str = ".source";
/// The line each setup run starts its part of `setup.log` with (see
/// `feature::setup`).
const SETUP_START: &str = "== Setup started";

/// How much of a chat is kept: `sessions/<agent>-<id>/meta.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub agent: AgentKind,
    pub id: String,
    /// Where it was copied from: a path here, or `<host>:<path>`.
    pub source: String,
    /// The machine it ran on.
    pub machine: String,
    /// How much of the transcript (or the database snapshot) is kept.
    pub raw_len: u64,
    /// The SHA-256 of what is kept: a transcript that grew from it gets
    /// only the rest appended.
    pub prefix_sha: String,
    /// Its source's last change, in milliseconds since the epoch: for
    /// Cursor, its database's or `-wal`'s, the later.
    pub mtime: u64,
    /// The frames of `transcript.jsonl.zst`.
    pub frames: u32,
    /// The length of `transcript.jsonl.zst` (or `store.db.zst`) once
    /// written: what is past it was cut short and is written again.
    #[serde(default)]
    pub packed: u64,
    /// All that was found of it was kept.
    pub complete: bool,
    /// The runs it was the chat of.
    pub runs: Vec<(Stage, u32)>,
    /// The files kept in `extra/`, by their path there.
    #[serde(default)]
    pub extras: BTreeMap<String, Kept>,
}

/// One file of `extra/` as it was kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub len: u64,
    /// In milliseconds; 0 for what was read out of a database.
    pub mtime: u64,
    pub sha: String,
}

/// The session folder of chat `id` of `agent` in feature folder `dir`.
pub fn session_dir(dir: &Path, agent: AgentKind, id: &str) -> PathBuf {
    dir.join("sessions")
        .join(format!("{}-{}", agent.name(), write::safe_name(id)))
}

/// The meta of each chat kept in feature folder `dir`.
pub fn metas(dir: &Path) -> Vec<Meta> {
    let Ok(entries) = std::fs::read_dir(dir.join("sessions")) else {
        return Vec::new();
    };
    (entries.flatten())
        .filter_map(|e| {
            (read_meta(&e.path()))
                .inspect_err(|err| tracing::warn!("reading {}: {err}", e.path().display()))
                .ok()
                .flatten()
        })
        .collect()
}

/// The meta of the chat kept in session folder `session`, if any.
pub fn read_meta(session: &Path) -> io::Result<Option<Meta>> {
    match std::fs::read(session.join(META)) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map(Some)
            .map_err(io::Error::other),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// `chat` with the id of the chat kept for its run already, when it came
/// without one (Codex, OpenCode): it is found by time once only.
pub fn with_known_id(dir: &Path, chat: &Chat) -> Chat {
    let mut chat = chat.clone();
    if chat.id.is_none() {
        let run = (chat.stage, chat.n);
        chat.id = (metas(dir).into_iter())
            .find(|m| m.agent == chat.agent && m.runs.contains(&run))
            .map(|m| m.id);
    }
    chat
}

/// The ids of `chat`'s agent that other runs of feature folder `dir` keep.
pub fn claimed(dir: &Path, chat: &Chat) -> HashSet<String> {
    let run = (chat.stage, chat.n);
    (metas(dir).into_iter())
        .filter(|m| m.agent == chat.agent && !m.runs.contains(&run))
        .map(|m| m.id)
        .collect()
}

/// When the file at `path` last changed, in milliseconds.
fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    (meta.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

fn sha(bytes: &[u8]) -> String {
    BlobRef::of(bytes).sha
}

/// Keep transcript `raw` in `session`: the rest appended as a frame when
/// it grew from what is kept, else all of it written again. Whether it
/// changed.
fn keep_lines(session: &Path, meta: &mut Meta, raw: &[u8]) -> io::Result<bool> {
    let path = session.join(TRANSCRIPT);
    let on_disk = std::fs::metadata(&path).map(|m| m.len()).ok();
    let len = raw.len() as u64;
    let kept = meta.frames > 0 && on_disk == Some(meta.packed);
    if kept && len == meta.raw_len && sha(raw) == meta.prefix_sha {
        return Ok(false);
    }
    let grew = kept && len > meta.raw_len && sha(&raw[..meta.raw_len as usize]) == meta.prefix_sha;
    if grew {
        meta.packed = zst::append_frame(&path, &raw[meta.raw_len as usize..], FRAME_LEVEL)?;
        meta.frames += 1;
    } else {
        let packed = zst::compress(raw, FRAME_LEVEL)?;
        write::atomic(&path, &packed)?;
        meta.packed = packed.len() as u64;
        meta.frames = 1;
    }
    meta.raw_len = len;
    meta.prefix_sha = sha(raw);
    Ok(true)
}

/// A copy of Cursor database `db`, whole and consistent, in `to`: it is
/// read while the agent may write it. A copy made from another machine,
/// `ours`, may be opened to write when reading it alone fails.
pub fn vacuum_into(db: &Path, to: &Path, ours: bool) -> Result<(), String> {
    // VACUUM INTO wants no file there; one it can't take away fails it,
    // and that is said.
    let _ = std::fs::remove_file(to);
    let copy = |conn: rusqlite::Connection| -> rusqlite::Result<()> {
        // Waits out a write of the agent's.
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute("VACUUM INTO ?1", [to.to_string_lossy()])?;
        Ok(())
    };
    let read_only = crate::cursor_agents::open_read_only(db).and_then(copy);
    let made = match read_only {
        Err(_) if ours => rusqlite::Connection::open(db).and_then(copy),
        other => other,
    };
    made.map_err(|err| format!("copying {}: {err}", db.display()))?;
    let check = || -> rusqlite::Result<String> {
        let conn = crate::cursor_agents::open_read_only(to)?;
        conn.query_row("PRAGMA quick_check", [], |row| row.get(0))
    };
    match check() {
        Ok(ok) if ok == "ok" => Ok(()),
        Ok(said) => Err(format!("the copy of {} is damaged: {said}", db.display())),
        Err(err) => Err(format!("checking the copy of {}: {err}", db.display())),
    }
}

/// Keep a Cursor chat's database `db` in `session`, unless it and its
/// `-wal` haven't moved since. Whether it changed.
fn keep_store(session: &Path, meta: &mut Meta, db: &Path, ours: bool) -> io::Result<bool> {
    let stamp = |path: &Path| std::fs::metadata(path).map(|m| mtime_ms(&m)).unwrap_or(0);
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    let mtime = stamp(db).max(stamp(Path::new(&wal)));
    let path = session.join(STORE);
    let on_disk = std::fs::metadata(&path).map(|m| m.len()).ok();
    if meta.frames > 0 && on_disk == Some(meta.packed) && mtime == meta.mtime {
        return Ok(false);
    }
    write::make_dir(session)?;
    let tmp = session.join(".store.db.tmp");
    let copied = vacuum_into(db, &tmp, ours)
        .map_err(io::Error::other)
        .and_then(|()| std::fs::read(&tmp));
    // The copy holds the chat unpacked: it goes at once, kept or not.
    let _ = std::fs::remove_file(&tmp);
    let raw = copied?;
    meta.mtime = mtime;
    let now = sha(&raw);
    if meta.frames > 0 && on_disk == Some(meta.packed) && now == meta.prefix_sha {
        return Ok(false);
    }
    let packed = zst::compress(&raw, FRAME_LEVEL)?;
    write::atomic(&path, &packed)?;
    meta.packed = packed.len() as u64;
    meta.frames = 1;
    meta.raw_len = raw.len() as u64;
    meta.prefix_sha = now;
    Ok(true)
}

/// Keep `bytes` as `extra/<rel>.zst` of `session` unless kept already.
/// Whether it changed.
fn keep_extra(
    session: &Path,
    meta: &mut Meta,
    rel: &str,
    bytes: &[u8],
    mtime: u64,
) -> io::Result<bool> {
    let now = sha(bytes);
    let len = bytes.len() as u64;
    if let Some(kept) = meta.extras.get_mut(rel)
        && kept.sha == now
    {
        kept.mtime = mtime;
        return Ok(false);
    }
    let path = session.join("extra").join(format!("{rel}.zst"));
    write::atomic(&path, &zst::compress(bytes, FRAME_LEVEL)?)?;
    meta.extras.insert(
        rel.to_owned(),
        Kept {
            len,
            mtime,
            sha: now,
        },
    );
    Ok(true)
}

/// Whether `rel` stays inside the folder it is a path in.
fn inside(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Keep chat `found`, of `run`, that ran on `machine`, in feature folder
/// `dir`; `pulled`: it is a copy made from another machine, from
/// `source`. The `Transcript` line to write when anything changed.
pub fn keep(
    dir: &Path,
    found: &Found,
    run: (Stage, u32),
    machine: &str,
    pulled: Option<&str>,
) -> io::Result<Option<Event>> {
    let session = session_dir(dir, found.agent, &found.id);
    let source = pulled.map_or_else(|| found.main.to_string_lossy().into_owned(), str::to_owned);
    let mut meta = read_meta(&session)?.unwrap_or_else(|| Meta {
        agent: found.agent,
        id: found.id.clone(),
        source: source.clone(),
        machine: machine.to_owned(),
        raw_len: 0,
        prefix_sha: String::new(),
        mtime: 0,
        frames: 0,
        packed: 0,
        complete: true,
        runs: Vec::new(),
        extras: BTreeMap::new(),
    });
    let before = meta.clone();
    let mut changed = false;
    let mut complete = true;
    match &found.form {
        Form::Lines => {
            let stat = std::fs::metadata(&found.main)?;
            let mtime = mtime_ms(&stat);
            let on_disk = std::fs::metadata(session.join(TRANSCRIPT))
                .map(|m| m.len())
                .ok();
            let same = meta.frames > 0
                && stat.len() == meta.raw_len
                && mtime == meta.mtime
                && on_disk == Some(meta.packed);
            if !same {
                let raw = std::fs::read(&found.main)?;
                changed |= keep_lines(&session, &mut meta, &raw)?;
                meta.mtime = mtime;
            }
        }
        Form::Made(raw) => changed |= keep_lines(&session, &mut meta, raw)?,
        Form::Store => changed |= keep_store(&session, &mut meta, &found.main, pulled.is_some())?,
        Form::Nothing => {}
    }
    for (rel, path) in &found.extra {
        if !inside(rel) {
            continue;
        }
        let read = std::fs::metadata(path).and_then(|stat| {
            let mtime = mtime_ms(&stat);
            if meta
                .extras
                .get(rel)
                .is_some_and(|k| k.len == stat.len() && k.mtime == mtime)
            {
                return Ok(false);
            }
            keep_extra(&session, &mut meta, rel, &std::fs::read(path)?, mtime)
        });
        match read {
            Ok(kept) => changed |= kept,
            // Gone between the listing and the read: the rest is kept.
            Err(err) if err.kind() == io::ErrorKind::NotFound => complete = false,
            Err(err) => return Err(err),
        }
    }
    for (rel, bytes) in &found.rows {
        if inside(rel) {
            changed |= keep_extra(&session, &mut meta, rel, bytes, 0)?;
        }
    }
    if !meta.runs.contains(&run) {
        meta.runs.push(run);
        changed = true;
    }
    if changed {
        meta.complete = complete;
        meta.source = source;
    }
    if meta != before {
        let text = serde_json::to_vec_pretty(&meta).map_err(io::Error::other)?;
        write::atomic(&session.join(META), &text)?;
    }
    Ok(changed.then(|| Event::Transcript {
        agent: meta.agent,
        session: meta.id.clone(),
        runs: meta.runs.clone(),
        raw_bytes: meta.raw_len,
        complete: meta.complete,
    }))
}

/// The part of `setup.log` text `raw` the last setup run wrote.
pub fn last_setup(raw: &[u8]) -> &[u8] {
    let text = String::from_utf8_lossy(raw);
    let at = (text.match_indices(SETUP_START))
        .filter(|(at, _)| *at == 0 || text.as_bytes()[at - 1] == b'\n')
        .last()
        .map_or(0, |(at, _)| at);
    // Lossy text is as long as the bytes up to the first bad one; past it,
    // the whole log is kept rather than a guess.
    match text.len() == raw.len() {
        true => &raw[at..],
        false => raw,
    }
}

/// What a pull on its own thread came to.
enum Back {
    /// The chat is in its staging folder: `Job::Pulled`.
    Pulled { key: Pull, job: Job },
    /// The chat isn't there.
    Gone {
        key: Pull,
        project: String,
        slug: String,
        chat: Chat,
    },
    /// The pull failed.
    Failed {
        key: Pull,
        what: String,
        why: String,
    },
}

/// A pull in flight: feature, agent, id, run.
type Pull = (String, String, AgentKind, Option<String>, Stage, u32);

/// The thread's state.
struct Worker {
    root: PathBuf,
    homes: Option<Homes>,
    machine: String,
    done: channel::Sender<Done>,
    back: mpsc::Sender<Back>,
    /// Set at the end: pulls and backfills stop.
    stop: Arc<AtomicBool>,
    /// A problem was said and nothing worked since.
    failing: bool,
    watch: Vec<(String, String, Chat)>,
    swept: Instant,
    index: Option<rusqlite::Connection>,
    /// Features with lines for the index, and since when.
    pending: BTreeSet<(String, String)>,
    since: Option<Instant>,
    pulling: HashSet<Pull>,
    /// Runs whose chat was said missing already.
    missing: HashSet<(String, String, Stage, u32)>,
}

/// Do `jobs` under `root` (see `super::root`) until told to stop, telling
/// `done` what they came to.
pub fn run(root: PathBuf, jobs: mpsc::Receiver<Job>, done: channel::Sender<Done>) {
    let (back, backs) = mpsc::channel();
    let mut worker = Worker::new(root, Homes::here(), super::machine(), done, back);
    worker.serve(&jobs, &backs);
}

impl Worker {
    fn new(
        root: PathBuf,
        homes: Option<Homes>,
        machine: String,
        done: channel::Sender<Done>,
        back: mpsc::Sender<Back>,
    ) -> Self {
        Self {
            root,
            homes,
            machine,
            done,
            back,
            stop: Arc::new(AtomicBool::new(false)),
            failing: false,
            watch: Vec::new(),
            swept: Instant::now(),
            index: None,
            pending: BTreeSet::new(),
            since: None,
            pulling: HashSet::new(),
            missing: HashSet::new(),
        }
    }

    fn serve(&mut self, jobs: &mpsc::Receiver<Job>, backs: &mpsc::Receiver<Back>) {
        let mut queue = VecDeque::new();
        loop {
            while let Ok(back) = backs.try_recv() {
                self.came_back(back);
            }
            let job = queue
                .pop_front()
                .or_else(|| match jobs.recv_timeout(self.wait()) {
                    Ok(job) => Some(job),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => Some(Job::Stop),
                });
            if let Some(job) = job
                && !self.handle(job, jobs, &mut queue)
            {
                break;
            }
            self.tick();
        }
        self.flush();
        self.stop.store(true, Ordering::Relaxed);
    }

    /// How long to wait for a job: until the batch is due, at most `TICK`.
    fn wait(&self) -> Duration {
        let due = self
            .since
            .map(|since| BATCH.saturating_sub(since.elapsed()));
        due.map_or(TICK, |due| due.min(TICK))
    }

    fn tick(&mut self) {
        if self.since.is_some_and(|since| since.elapsed() >= BATCH) {
            self.flush();
        }
        if self.swept.elapsed() >= SWEEP {
            self.swept = Instant::now();
            self.sweep();
        }
    }

    /// Do `job`; false: stop.
    fn handle(&mut self, job: Job, jobs: &mpsc::Receiver<Job>, queue: &mut VecDeque<Job>) -> bool {
        match job {
            Job::Index { project, slug } => self.index_later(&project, &slug),
            Job::Capture {
                project,
                slug,
                chat,
                there: Some((reach, _side)),
            } => self.capture_there(&project, &slug, chat, reach),
            Job::Capture {
                project,
                slug,
                chat,
                there: None,
            } => self.capture_here(&project, &slug, &chat, false),
            Job::Outputs {
                project,
                slug,
                files,
                reason,
            } => self.outputs(&project, &slug, &files, &reason),
            Job::SetupSlice {
                project,
                slug,
                run,
                log,
            } => self.setup_slice(&project, &slug, run, &log),
            Job::Watch { chats } => self.watch = chats,
            Job::Rescan => self.rescan(),
            Job::Backfill { features } => return self.backfill(&features, jobs, queue),
            Job::Pulled {
                project,
                slug,
                chat,
                staging,
            } => self.pulled(&project, &slug, &chat, &staging),
            Job::Stop => return false,
        }
        true
    }

    fn report(&self, report: Report) {
        // Gone only when the loop is: there is no one left to tell.
        let _ = self.done.send(Done::AiWork(report));
    }

    fn failed(&mut self, what: &str, why: String) {
        tracing::warn!("ai-work: {what}: {why}");
        self.failing = true;
        self.report(Report::Failed {
            what: what.to_owned(),
            why,
        });
    }

    fn worked(&mut self) {
        if std::mem::take(&mut self.failing) {
            self.report(Report::Worked);
        }
    }

    fn dir(&self, project: &str, slug: &str) -> PathBuf {
        self.root.join(project).join(slug)
    }

    /// Write `event` to feature `slug`'s events, for the index soon.
    fn say(&mut self, project: &str, slug: &str, event: Event) {
        let line = Line {
            v: FORMAT,
            at_ms: now_ms(),
            machine: self.machine.clone(),
            backfill: false,
            event,
        };
        match write::append(&self.dir(project, slug), &line) {
            Ok(()) => {
                self.worked();
                self.index_later(project, slug);
            }
            Err(err) => self.failed("Couldn't keep AI work", format!("{project}/{slug}: {err}")),
        }
    }

    fn index_later(&mut self, project: &str, slug: &str) {
        self.pending.insert((project.to_owned(), slug.to_owned()));
        self.since.get_or_insert_with(Instant::now);
    }

    /// Take the pending features' new lines into the index.
    fn flush(&mut self) {
        self.since = None;
        let dirs: Vec<PathBuf> = (std::mem::take(&mut self.pending).iter())
            .map(|(project, slug)| self.dir(project, slug))
            .collect();
        if !dirs.is_empty() {
            self.ingest(&dirs);
        }
    }

    fn ingest(&mut self, dirs: &[PathBuf]) {
        let conn = match self.index.take() {
            Some(conn) => Ok(conn),
            None => super::index::open(&self.root),
        };
        let mut conn = match conn {
            Ok(conn) => conn,
            Err(why) => return self.failed("Couldn't update the AI work stats", why),
        };
        match super::index::ingest(&mut conn, dirs) {
            Ok(lines) => {
                self.index = Some(conn);
                self.worked();
                if lines > 0 {
                    self.report(Report::Indexed { lines });
                }
            }
            // Opened again next time, in case it was what failed.
            Err(why) => self.failed("Couldn't update the AI work stats", why),
        }
    }

    /// Every feature folder under the root.
    fn feature_dirs(&self) -> Vec<PathBuf> {
        let subdirs = |dir: &Path| -> Vec<PathBuf> {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Vec::new();
            };
            let mut dirs: Vec<PathBuf> = (entries.flatten())
                .filter(|e| e.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.path())
                .collect();
            dirs.sort();
            dirs
        };
        (subdirs(&self.root).iter())
            .flat_map(|project| subdirs(project))
            .collect()
    }

    fn rescan(&mut self) {
        self.index = None;
        let path = super::index::path(&self.root);
        for end in ["", "-wal", "-shm"] {
            let mut file = path.as_os_str().to_owned();
            file.push(end);
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    let why = format!("{}: {err}", Path::new(&file).display());
                    return self.failed("Couldn't draw the AI work stats again", why);
                }
            }
        }
        self.pending.clear();
        self.since = None;
        let dirs = self.feature_dirs();
        self.ingest(&dirs);
    }

    fn missing(&mut self, project: &str, slug: &str, chat: &Chat, why: &str) {
        let key = (project.to_owned(), slug.to_owned(), chat.stage, chat.n);
        if !self.missing.insert(key) {
            return;
        }
        let event = Event::TranscriptMissing {
            agent: chat.agent,
            session: chat.id.clone(),
            stage: chat.stage,
            n: chat.n,
            why: why.to_owned(),
        };
        self.say(project, slug, event);
    }

    /// Keep `found`, `chat`'s, in feature `slug`; `pulled`: from another
    /// machine, from there.
    fn keep(
        &mut self,
        project: &str,
        slug: &str,
        chat: &Chat,
        found: &Found,
        pulled: Option<&str>,
    ) {
        let machine = chat.machine.clone().unwrap_or_else(|| self.machine.clone());
        let dir = self.dir(project, slug);
        match keep(&dir, found, (chat.stage, chat.n), &machine, pulled) {
            Ok(Some(event)) => {
                self.missing
                    .remove(&(project.to_owned(), slug.to_owned(), chat.stage, chat.n));
                self.say(project, slug, event);
            }
            Ok(None) => {}
            Err(err) => {
                let what = format!(
                    "Couldn't keep the chat of {} {}",
                    chat.stage.label(),
                    chat.n
                );
                self.failed(&what, format!("{}: {err}", found.main.display()));
            }
        }
    }

    /// Keep `chat` of feature `slug` from this machine; `quiet`: a chat not
    /// there yet isn't said missing (the sweep).
    fn capture_here(&mut self, project: &str, slug: &str, chat: &Chat, quiet: bool) {
        let Some(homes) = self.homes.clone() else {
            let what = "Couldn't keep the agents' chats";
            return self.failed(what, "this machine has no home folder (HOME)".to_owned());
        };
        let dir = self.dir(project, slug);
        let chat = with_known_id(&dir, chat);
        match locate::find(&homes, &chat, &claimed(&dir, &chat)) {
            Ok(Some(found)) => self.keep(project, slug, &chat, &found, None),
            Ok(None) if quiet => {}
            Ok(None) => self.missing(project, slug, &chat, "not found on this machine"),
            Err(why) => {
                let what = format!(
                    "Couldn't find the chat of {} {}",
                    chat.stage.label(),
                    chat.n
                );
                self.failed(&what, why);
            }
        }
    }

    /// Keep the chats at work, those of this machine: the others are kept
    /// when their runs change.
    fn sweep(&mut self) {
        let chats: Vec<_> = (self.watch.iter())
            .filter(|(_, _, chat)| chat.machine.is_none())
            .cloned()
            .collect();
        for (project, slug, chat) in chats {
            self.capture_here(&project, &slug, &chat, true);
        }
    }

    /// Pull `chat` from the machine `reach` reaches, on a thread of its
    /// own: a big chat would hold up the rest.
    fn capture_there(&mut self, project: &str, slug: &str, chat: Chat, reach: Reach) {
        let dir = self.dir(project, slug);
        let chat = with_known_id(&dir, &chat);
        let key: Pull = (
            project.to_owned(),
            slug.to_owned(),
            chat.agent,
            chat.id.clone(),
            chat.stage,
            chat.n,
        );
        if self.pulling.contains(&key) {
            return;
        }
        let Some(script) = locate::script(&chat) else {
            return self.missing(project, slug, &chat, "not kept from another machine");
        };
        let claimed = claimed(&dir, &chat);
        let staging = self
            .root
            .join(".staging")
            .join(write::safe_name(&reach.target));
        let (back, stop) = (self.back.clone(), self.stop.clone());
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let spawned = std::thread::Builder::new()
            .name("ai-work-pull".into())
            .spawn({
                let key = key.clone();
                move || {
                    let what = format!(
                        "Couldn't copy the chat of {} {}",
                        chat.stage.label(),
                        chat.n
                    );
                    let back_with = match pull(&reach, &chat, &script, &claimed, &staging, &stop) {
                        Ok(Some((id, staging))) => {
                            let chat = Chat {
                                id: Some(id),
                                ..chat
                            };
                            let job = Job::Pulled {
                                project,
                                slug,
                                chat,
                                staging,
                            };
                            Back::Pulled { key, job }
                        }
                        Ok(None) => Back::Gone {
                            key,
                            project,
                            slug,
                            chat,
                        },
                        Err(why) => Back::Failed { key, what, why },
                    };
                    // Gone when the thread ended: what was pulled waits in
                    // staging for the next capture.
                    let _ = back.send(back_with);
                }
            });
        match spawned {
            Ok(_) => {
                self.pulling.insert(key);
            }
            Err(err) => self.failed("Couldn't copy an agent's chat", err.to_string()),
        }
    }

    fn came_back(&mut self, back: Back) {
        match back {
            Back::Pulled { key, job } => {
                self.pulling.remove(&key);
                if let Job::Pulled {
                    project,
                    slug,
                    chat,
                    staging,
                } = job
                {
                    self.pulled(&project, &slug, &chat, &staging);
                }
            }
            Back::Gone {
                key,
                project,
                slug,
                chat,
            } => {
                self.pulling.remove(&key);
                self.missing(&project, &slug, &chat, "not found on its machine");
            }
            Back::Failed { key, what, why } => {
                self.pulling.remove(&key);
                self.failed(&what, why);
            }
        }
    }

    fn pulled(&mut self, project: &str, slug: &str, chat: &Chat, staging: &Path) {
        let found = chat
            .id
            .as_deref()
            .and_then(|id| locate::staged(chat.agent, id, staging));
        let Some(found) = found else {
            return self.missing(project, slug, chat, "not found on its machine");
        };
        let source = std::fs::read_to_string(staging.join(SOURCE))
            .unwrap_or_else(|_| staging.to_string_lossy().into_owned());
        self.keep(project, slug, chat, &found, Some(source.trim()));
    }

    fn outputs(&mut self, project: &str, slug: &str, files: &[(String, PathBuf)], reason: &str) {
        let dir = self.dir(project, slug);
        let what = "Couldn't keep the stage files";
        let mut marks = match write::read_marks(&dir) {
            Ok(marks) => marks,
            Err(err) => {
                return self.failed(
                    what,
                    format!("{}: {err}", write::marks_path(&dir).display()),
                );
            }
        };
        let mut events = Vec::new();
        for (name, path) in files {
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                // Not written (yet): there is nothing to keep.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => {
                    self.failed(what, format!("{}: {err}", path.display()));
                    continue;
                }
            };
            let blob = BlobRef::of(&bytes);
            if marks.outputs.get(name) == Some(&blob.sha) {
                continue;
            }
            if let Err(err) = blobs::put(&dir, &bytes) {
                self.failed(what, format!("{}: {err}", path.display()));
                continue;
            }
            marks.outputs.insert(name.clone(), blob.sha.clone());
            events.push(Event::Output {
                file: name.clone(),
                blob,
                reason: reason.to_owned(),
            });
        }
        if events.is_empty() {
            return;
        }
        for event in events {
            self.say(project, slug, event);
        }
        if let Err(err) = write::write_marks(&dir, &marks) {
            self.failed(
                what,
                format!("{}: {err}", write::marks_path(&dir).display()),
            );
        }
    }

    fn setup_slice(&mut self, project: &str, slug: &str, run: Value, log: &Path) {
        let dir = self.dir(project, slug);
        let blob = match std::fs::read(log) {
            Ok(raw) => match blobs::put(&dir, last_setup(&raw)) {
                Ok(blob) => Some(blob),
                Err(err) => {
                    self.failed(
                        "Couldn't keep the setup log",
                        format!("{}: {err}", dir.display()),
                    );
                    None
                }
            },
            // The setup ended without a log: the run is noted alone.
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => {
                self.failed(
                    "Couldn't keep the setup log",
                    format!("{}: {err}", log.display()),
                );
                None
            }
        };
        self.say(project, slug, Event::SetupEnded { run, log: blob });
    }

    /// Keep the earlier AI work of `features`, one at a time; a `Stop`
    /// between two ends it. False: stop.
    fn backfill(
        &mut self,
        features: &[(String, String)],
        jobs: &mpsc::Receiver<Job>,
        queue: &mut VecDeque<Job>,
    ) -> bool {
        let data = self
            .root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let shared = data == crate::projects::data_dir();
        let total = features.len();
        let (mut kept, mut gone) = (0, 0);
        for (i, (project, slug)) in features.iter().enumerate() {
            while let Ok(job) = jobs.try_recv() {
                if matches!(job, Job::Stop) {
                    self.stop.store(true, Ordering::Relaxed);
                }
                queue.push_back(job);
            }
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            // The data folder this gerdr reads goes by `store::dir`, which
            // knows the copies of other machines' features.
            let feature =
                crate::feature::store::dir((!shared).then_some(data.as_path()), project, slug);
            let dir = self.dir(project, slug);
            match super::backfill::feature(&feature, &dir, &self.stop) {
                Ok(done) => {
                    kept += 1;
                    gone += done.gone;
                    self.worked();
                    self.index_later(project, slug);
                }
                Err(why) => self.failed(
                    "Couldn't keep earlier AI work",
                    format!("{project}/{slug}: {why}"),
                ),
            }
            self.report(Report::Backfilling { done: i + 1, total });
        }
        self.report(Report::Backfilled {
            features: kept,
            gone,
        });
        !self.stop.load(Ordering::Relaxed)
    }
}

/// Pull `chat` with `script` from the machine `reach` reaches into its
/// folder under `staging`: its id and that folder, or None when it isn't
/// there.
fn pull(
    reach: &Reach,
    chat: &Chat,
    script: &str,
    claimed: &HashSet<String>,
    staging: &Path,
    stop: &AtomicBool,
) -> Result<Option<(String, PathBuf)>, String> {
    let out = reach
        .script(script, stop)
        .map_err(|t| t.text().to_owned())?;
    let Some(listing) = locate::read_listing(chat, &out, claimed) else {
        return Ok(None);
    };
    let folder = staging.join(format!(
        "{}-{}",
        listing.agent.name(),
        write::safe_name(&listing.id)
    ));
    let made =
        |path: &Path| write::make_dir(path).map_err(|err| format!("{}: {err}", path.display()));
    made(&folder)?;
    let get = |local: &Path, there: &str| {
        if let Some(parent) = local.parent() {
            made(parent)?;
        }
        reach
            .pull_file(local, there, stop)
            .map_err(|t| t.text().to_owned())
    };
    get(
        &locate::staged_main(listing.agent, &folder),
        &listing.main.path,
    )?;
    let wal = folder.join("store.db-wal");
    match &listing.wal {
        Some(remote) => get(&wal, &remote.path)?,
        // One left from before would be read with the new database.
        None => match std::fs::remove_file(&wal) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("{}: {err}", wal.display())),
        },
    }
    for (rel, remote) in &listing.extra {
        get(&folder.join("extra").join(rel), &remote.path)?;
    }
    let source = format!("{}:{}", reach.target, listing.main.path);
    write::atomic(&folder.join(SOURCE), source.as_bytes())
        .map_err(|err| format!("{}: {err}", folder.display()))?;
    Ok(Some((listing.id, folder)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::write::tests::scratch;

    fn chat(agent: AgentKind, id: Option<&str>) -> Chat {
        Chat {
            agent,
            id: id.map(str::to_owned),
            stage: Stage::Build,
            n: 1,
            started: 0,
            folders: Vec::new(),
            machine: None,
        }
    }

    fn lines_found(main: &Path) -> Found {
        Found {
            agent: AgentKind::Claude,
            id: "c-1".into(),
            main: main.to_path_buf(),
            extra: Vec::new(),
            form: Form::Lines,
            rows: Vec::new(),
        }
    }

    /// Set `path`'s mtime `secs` after the epoch, so a test's writes within
    /// one tick still look changed.
    fn touch(path: &Path, secs: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn frames_of(dir: &Path) -> (Meta, Vec<u8>) {
        let session = session_dir(dir, AgentKind::Claude, "c-1");
        let meta = read_meta(&session).unwrap().unwrap();
        (meta, zst::read(&session.join(TRANSCRIPT)).unwrap())
    }

    #[test]
    fn a_growing_transcript_gets_frames_appended() {
        let root = scratch("thread-grow");
        let (dir, main) = (root.join("p/s"), root.join("c-1.jsonl"));
        std::fs::write(&main, "{\"a\":1}\n").unwrap();
        touch(&main, 100);
        let found = lines_found(&main);
        let event = keep(&dir, &found, (Stage::Build, 1), "m", None).unwrap();
        assert!(matches!(
            event,
            Some(Event::Transcript { raw_bytes: 8, .. })
        ));
        std::fs::write(&main, "{\"a\":1}\n{\"b\":2}\n").unwrap();
        touch(&main, 200);
        keep(&dir, &found, (Stage::Build, 1), "m", None)
            .unwrap()
            .unwrap();
        let (meta, raw) = frames_of(&dir);
        assert_eq!(meta.frames, 2);
        assert_eq!(raw, b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(meta.raw_len, 16);
        assert_eq!(meta.runs, vec![(Stage::Build, 1)]);
    }

    #[test]
    fn a_changed_start_writes_it_all_again() {
        let root = scratch("thread-rewrite");
        let (dir, main) = (root.join("p/s"), root.join("c-1.jsonl"));
        std::fs::write(&main, "{\"a\":1}\n").unwrap();
        touch(&main, 100);
        let found = lines_found(&main);
        keep(&dir, &found, (Stage::Build, 1), "m", None).unwrap();
        std::fs::write(&main, "{\"z\":9}\n{\"b\":2}\n").unwrap();
        touch(&main, 200);
        keep(&dir, &found, (Stage::Build, 1), "m", None)
            .unwrap()
            .unwrap();
        let (meta, raw) = frames_of(&dir);
        assert_eq!(meta.frames, 1);
        assert_eq!(raw, b"{\"z\":9}\n{\"b\":2}\n");
    }

    #[test]
    fn an_unchanged_chat_does_nothing() {
        let root = scratch("thread-same");
        let (dir, main) = (root.join("p/s"), root.join("c-1.jsonl"));
        std::fs::write(&main, "{\"a\":1}\n").unwrap();
        touch(&main, 100);
        let found = lines_found(&main);
        keep(&dir, &found, (Stage::Build, 1), "m", None).unwrap();
        let session = session_dir(&dir, AgentKind::Claude, "c-1");
        let stamp = |name: &str| {
            std::fs::metadata(session.join(name))
                .unwrap()
                .modified()
                .unwrap()
        };
        let (meta, packed) = (stamp(META), stamp(TRANSCRIPT));
        assert_eq!(
            keep(&dir, &found, (Stage::Build, 1), "m", None).unwrap(),
            None
        );
        // Touched only, not changed: still nothing.
        touch(&main, 300);
        assert_eq!(
            keep(&dir, &found, (Stage::Build, 1), "m", None).unwrap(),
            None
        );
        assert_eq!(stamp(TRANSCRIPT), packed);
        assert_ne!(stamp(META), meta, "the new mtime is kept");
        // A new run of the same chat is news.
        let event = keep(&dir, &found, (Stage::Build, 2), "m", None)
            .unwrap()
            .unwrap();
        let Event::Transcript { runs, .. } = event else {
            panic!("{event:?}")
        };
        assert_eq!(runs, vec![(Stage::Build, 1), (Stage::Build, 2)]);
    }

    #[test]
    fn a_frame_cut_short_is_written_again() {
        let root = scratch("thread-torn");
        let (dir, main) = (root.join("p/s"), root.join("c-1.jsonl"));
        std::fs::write(&main, "{\"a\":1}\n").unwrap();
        touch(&main, 100);
        let found = lines_found(&main);
        keep(&dir, &found, (Stage::Build, 1), "m", None).unwrap();
        let session = session_dir(&dir, AgentKind::Claude, "c-1");
        // A crash in the middle of an append left half a frame.
        write::open_append(&session.join(TRANSCRIPT))
            .and_then(|mut f| io::Write::write_all(&mut f, b"\x28\xb5"))
            .unwrap();
        std::fs::write(&main, "{\"a\":1}\n{\"b\":2}\n").unwrap();
        touch(&main, 200);
        keep(&dir, &found, (Stage::Build, 1), "m", None)
            .unwrap()
            .unwrap();
        let (meta, raw) = frames_of(&dir);
        assert_eq!(
            (meta.frames, raw.as_slice()),
            (1, &b"{\"a\":1}\n{\"b\":2}\n"[..])
        );
    }

    #[test]
    fn claude_extras_are_kept_once() {
        let root = scratch("thread-extras");
        let (dir, main) = (root.join("p/s"), root.join("c-1.jsonl"));
        std::fs::write(&main, "{}\n").unwrap();
        let tool = root.join("t1.txt");
        std::fs::write(&tool, "output").unwrap();
        let mut found = lines_found(&main);
        found.extra = vec![
            ("tool-results/t1.txt".into(), tool.clone()),
            ("../escape".into(), tool.clone()),
            ("subagents/gone.jsonl".into(), root.join("gone")),
        ];
        let event = keep(&dir, &found, (Stage::Plan, 1), "m", None)
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            Event::Transcript {
                complete: false,
                ..
            }
        ));
        let session = session_dir(&dir, AgentKind::Claude, "c-1");
        let kept = session.join("extra/tool-results/t1.txt.zst");
        assert_eq!(zst::read(&kept).unwrap(), b"output");
        assert!(!dir.join("sessions/escape.zst").exists());
        let meta = read_meta(&session).unwrap().unwrap();
        assert_eq!(meta.extras.len(), 1);
        found.extra.truncate(1);
        assert_eq!(
            keep(&dir, &found, (Stage::Plan, 1), "m", None).unwrap(),
            None
        );
    }

    #[test]
    fn kept_chats_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = scratch("thread-modes");
        let (dir, main) = (root.join("p/s"), root.join("c-1.jsonl"));
        std::fs::write(&main, "{}\n").unwrap();
        keep(&dir, &lines_found(&main), (Stage::Plan, 1), "m", None).unwrap();
        let session = session_dir(&dir, AgentKind::Claude, "c-1");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        for folder in [&dir, &dir.join("sessions"), &session] {
            assert_eq!(mode(folder), 0o700, "{}", folder.display());
        }
        for file in [META, TRANSCRIPT] {
            assert_eq!(mode(&session.join(file)), 0o600, "{file}");
        }
    }

    /// A Cursor-like `store.db` in WAL mode, as it is while the agent runs.
    fn cursor_db(path: &Path, rows: usize) -> rusqlite::Connection {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        for i in 0..rows {
            conn.execute(
                "INSERT OR REPLACE INTO blobs VALUES (?1, ?2)",
                rusqlite::params![format!("b{i}"), vec![7u8; 64]],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn a_cursor_store_is_copied_whole_while_open() {
        let root = scratch("thread-cursor");
        let dir = root.join("p/s");
        let db = root.join("chats/x/k-1/store.db");
        // Held open, with what it wrote still in its -wal.
        let conn = cursor_db(&db, 3);
        let found = Found {
            agent: AgentKind::Cursor,
            id: "k-1".into(),
            main: db.clone(),
            extra: Vec::new(),
            form: Form::Store,
            rows: Vec::new(),
        };
        keep(&dir, &found, (Stage::Build, 1), "m", None)
            .unwrap()
            .unwrap();
        let session = session_dir(&dir, AgentKind::Cursor, "k-1");
        let copy = root.join("copy.db");
        std::fs::write(&copy, zst::read(&session.join(STORE)).unwrap()).unwrap();
        let count = |path: &Path| -> i64 {
            rusqlite::Connection::open(path)
                .unwrap()
                .query_row("SELECT count(*) FROM blobs", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count(&copy), 3);
        assert!(!session.join(".store.db.tmp").exists());
        // Nothing moved: not copied again.
        assert_eq!(
            keep(&dir, &found, (Stage::Build, 1), "m", None).unwrap(),
            None
        );
        cursor_db(&db, 5);
        let mut wal = db.as_os_str().to_owned();
        wal.push("-wal");
        touch(Path::new(&wal), 4_000_000_000);
        keep(&dir, &found, (Stage::Build, 1), "m", None)
            .unwrap()
            .unwrap();
        std::fs::write(&copy, zst::read(&session.join(STORE)).unwrap()).unwrap();
        assert_eq!(count(&copy), 5);
        drop(conn);
    }

    #[test]
    fn a_pulled_cursor_store_without_its_shm_is_copied() {
        let root = scratch("thread-cursor-pulled");
        let db = root.join("there/store.db");
        let conn = cursor_db(&db, 2);
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::copy(&db, staging.join("store.db")).unwrap();
        std::fs::copy(
            root.join("there/store.db-wal"),
            staging.join("store.db-wal"),
        )
        .unwrap();
        drop(conn);
        let to = root.join("out.db");
        vacuum_into(&staging.join("store.db"), &to, true).unwrap();
        let n: i64 = rusqlite::Connection::open(&to)
            .unwrap()
            .query_row("SELECT count(*) FROM blobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    fn worker(root: &Path) -> (Worker, channel::Channel<Done>) {
        let (done, dones) = channel::channel();
        let (back, _) = mpsc::channel();
        let homes = Homes::under(&root.join("home"));
        (
            Worker::new(root.to_path_buf(), Some(homes), "m1".into(), done, back),
            dones,
        )
    }

    fn events(dir: &Path) -> Vec<Line> {
        let text = std::fs::read_to_string(dir.join("events-m1.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn stage_files_are_kept_when_they_change() {
        let root = scratch("thread-outputs");
        let (mut w, _dones) = worker(&root);
        let plan = root.join("plan.md");
        std::fs::write(&plan, "v1").unwrap();
        let files = vec![
            ("plan.md".to_owned(), plan.clone()),
            ("build.md".to_owned(), root.join("not-yet.md")),
        ];
        w.outputs("p", "s", &files, "ready");
        w.outputs("p", "s", &files, "ready");
        std::fs::write(&plan, "v2").unwrap();
        w.outputs("p", "s", &files, "stop");
        let dir = root.join("p/s");
        let kept: Vec<(String, String)> = (events(&dir).into_iter())
            .map(|l| match l.event {
                Event::Output { file, blob, reason } => {
                    assert_eq!(blobs::get(&dir, &blob).unwrap().len(), 2);
                    (file, reason)
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kept,
            [
                ("plan.md".into(), "ready".into()),
                ("plan.md".into(), "stop".into())
            ]
        );
        assert_eq!(w.pending.len(), 1, "queued for the index");
    }

    #[test]
    fn a_capture_finds_keeps_and_says_missing_once() {
        let root = scratch("thread-capture");
        let (mut w, _dones) = worker(&root);
        let transcript = root.join("home/.claude/projects/-w/c-1.jsonl");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "{}\n").unwrap();
        w.capture_here("p", "s", &chat(AgentKind::Claude, Some("c-1")), false);
        w.capture_here("p", "s", &chat(AgentKind::Claude, Some("gone")), false);
        w.capture_here("p", "s", &chat(AgentKind::Claude, Some("gone")), false);
        let kinds: Vec<_> = (events(&root.join("p/s")).into_iter())
            .map(|l| match l.event {
                Event::Transcript { session, .. } => format!("kept {session}"),
                Event::TranscriptMissing { session, .. } => format!("missing {session:?}"),
                other => panic!("{other:?}"),
            })
            .collect();
        // The second run, with the same stage and number, is said once.
        assert_eq!(kinds, ["kept c-1", "missing Some(\"gone\")"]);
    }

    #[test]
    fn the_setup_slice_is_the_last_run() {
        let log = b"== Setup started 1\nold\n== Setup started 2\nnew\n";
        assert_eq!(last_setup(log), b"== Setup started 2\nnew\n");
        assert_eq!(last_setup(b"no runs"), b"no runs");
        let root = scratch("thread-setup");
        let (mut w, _dones) = worker(&root);
        let path = root.join("setup.log");
        std::fs::write(&path, log).unwrap();
        w.setup_slice("p", "s", serde_json::json!({"n": 2}), &path);
        let dir = root.join("p/s");
        let [line] = events(&dir).try_into().unwrap();
        let Event::SetupEnded {
            log: Some(blob), ..
        } = line.event
        else {
            panic!("{:?}", line.event)
        };
        assert_eq!(
            blobs::get(&dir, &blob).unwrap(),
            b"== Setup started 2\nnew\n"
        );
    }

    #[test]
    fn failures_are_said_then_cleared() {
        let root = scratch("thread-fail");
        let (mut w, dones) = worker(&root);
        // A file where the feature's folder should be.
        std::fs::create_dir_all(root.join("p")).unwrap();
        std::fs::write(root.join("p/s"), "").unwrap();
        w.say("p", "s", Event::Unmarked { stage: Stage::Plan });
        assert!(matches!(
            dones.try_recv(),
            Ok(Done::AiWork(Report::Failed { .. }))
        ));
        w.say("p", "t", Event::Unmarked { stage: Stage::Plan });
        assert!(matches!(dones.try_recv(), Ok(Done::AiWork(Report::Worked))));
        w.say("p", "t", Event::Unmarked { stage: Stage::Plan });
        assert!(dones.try_recv().is_err(), "said once");
    }

    #[test]
    fn the_thread_stops_when_told() {
        let root = scratch("thread-stop");
        let (done, _dones) = channel::channel();
        let (jobs, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || run(root, rx, done));
        jobs.send(Job::Watch { chats: Vec::new() }).unwrap();
        jobs.send(Job::Stop).unwrap();
        handle.join().unwrap();
    }
}
