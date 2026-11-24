//! A thread of its own that finds the chats of the panes shown as chats,
//! reads their transcripts as they grow and sends their items back.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use serde_json::Value;
use smithay::reexports::calloop::channel;

use super::{
    Event, Item, Source,
    parse::{self, Reader},
    source::{self, Run},
};
use crate::{
    ai_work::{AgentKind, locate::Homes},
    herdr::{AgentInfo, Herdr},
};

/// The most of a transcript read the first time, its end; and how much
/// more each ask for earlier messages reads.
pub const CHAT_TAIL: u64 = 2 * 1024 * 1024;
/// How often the followed chats are looked at.
const POLL: Duration = Duration::from_millis(500);
/// How often a chat not found yet is looked for again: that lists the
/// agent's folders of chats.
const FIND_AGAIN: Duration = Duration::from_secs(5);
/// The bytes kept from before the read offset: when they change, the file
/// was written anew and is read again from the start.
const PREV: usize = 4096;
/// How long herdr's list of its integrations is trusted: one installed
/// meanwhile shows after this.
const INTEGRATIONS_AGAIN: Duration = Duration::from_secs(180);
/// Where the problems of chats go, by herdr's integration (`cursor`).
const PROBLEMS: &str = "chat";
/// The state herdr gives an integration it hasn't installed.
const NOT_INSTALLED: &str = "not_installed";

enum Cmd {
    Follow(Box<AgentInfo>),
    Stop(String),
    Earlier(String),
    Read(String),
}

/// The thread: what it is asked to follow.
pub struct Tail {
    cmds: mpsc::Sender<Cmd>,
}

impl Tail {
    /// Start the thread, sending what it reads to `out`. It asks the herdr
    /// on `socket` what runs in a pane whose chat herdr doesn't name.
    pub fn spawn(out: channel::Sender<Event>, socket: Option<PathBuf>) -> Self {
        let (cmds, asked) = mpsc::channel();
        std::thread::spawn(move || {
            let homes = Homes::here().unwrap_or_else(|| {
                tracing::warn!("agent chats: no home folder, so no chat is found");
                Homes {
                    claude: Vec::new(),
                    ..Homes::under(Path::new("/"))
                }
            });
            let mut chats = Chats::new(homes, socket.map(Herdr::on_socket));
            run(&asked, &mut chats, &out);
        });
        Self { cmds }
    }

    /// Follow the chat of the agent in pane `agent.pane_id`: find it, read
    /// it, and send its items each time it grows. Asked again for the same
    /// pane (herdr reports another session), it finds the chat again.
    pub fn follow(&self, agent: AgentInfo) {
        self.send(Cmd::Follow(Box::new(agent)));
    }

    /// Stop following the chat of pane `pane_id` (hidden or closed).
    pub fn stop(&self, pane_id: &str) {
        self.send(Cmd::Stop(pane_id.to_owned()));
    }

    /// Read more of the chat of pane `pane_id`, before what was read, and
    /// send all its items again.
    pub fn earlier(&self, pane_id: &str) {
        self.send(Cmd::Earlier(pane_id.to_owned()));
    }

    /// Read the chat of pane `pane_id` now: its agent said something
    /// (`live`).
    pub fn read(&self, pane_id: &str) {
        self.send(Cmd::Read(pane_id.to_owned()));
    }

    fn send(&self, cmd: Cmd) {
        if self.cmds.send(cmd).is_err() {
            tracing::warn!("agent chats: the thread that reads them has ended");
        }
    }
}

/// The thread's loop: until the `Tail` is dropped or gerdr stops listening.
fn run(asked: &mpsc::Receiver<Cmd>, chats: &mut Chats, out: &channel::Sender<Event>) {
    let mut next = Instant::now() + POLL;
    loop {
        // Nothing followed: nothing to poll, wait for a command.
        let cmd = match chats.panes.is_empty() {
            true => asked
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            false => asked.recv_timeout(next.saturating_duration_since(Instant::now())),
        };
        let mut events = Vec::new();
        match cmd {
            Ok(Cmd::Follow(agent)) => {
                if chats.panes.is_empty() {
                    next = Instant::now() + POLL;
                }
                chats.follow(*agent, &mut events);
            }
            Ok(Cmd::Stop(pane_id)) => chats.stop(&pane_id),
            Ok(Cmd::Earlier(pane_id)) => chats.earlier(&pane_id, &mut events),
            Ok(Cmd::Read(pane_id)) => chats.read_now(&pane_id, &mut events),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                chats.poll(&mut events);
                next = Instant::now() + POLL;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        for event in events {
            if out.send(event).is_err() {
                return;
            }
        }
    }
}

/// The panes followed, and their chats.
struct Chats {
    homes: Homes,
    /// The herdr asked what runs in a pane; none in tests.
    herdr: Option<Herdr>,
    /// herdr's integrations' states by target (`cursor`: `not_installed`),
    /// and when it was asked.
    integrations: Option<(Instant, HashMap<String, String>)>,
    panes: HashMap<String, Followed>,
}

struct Followed {
    agent: AgentInfo,
    /// When its chat was last looked for.
    looked: Instant,
    /// Its chat, or why it has none (told the user once).
    chat: Result<Chat, String>,
}

impl Chats {
    fn new(homes: Homes, herdr: Option<Herdr>) -> Self {
        Self {
            homes,
            herdr,
            integrations: None,
            panes: HashMap::new(),
        }
    }

    fn follow(&mut self, agent: AgentInfo, out: &mut Vec<Event>) {
        let found = self.find(&agent);
        if let Some(had) = self.panes.get_mut(&agent.pane_id) {
            let same = match (&had.chat, &found) {
                (Ok(chat), Ok(source)) => chat.source == *source,
                (Err(why), Err(now)) => why == now,
                _ => false,
            };
            if same {
                had.agent = agent;
                return;
            }
        }
        let pane_id = agent.pane_id.clone();
        let mut followed = Followed {
            chat: Err(String::new()),
            agent,
            looked: Instant::now(),
        };
        followed.found(found, out);
        self.panes.insert(pane_id, followed);
    }

    fn stop(&mut self, pane_id: &str) {
        self.panes.remove(pane_id);
    }

    /// Read more of pane `pane_id`'s chat before what was read.
    fn earlier(&mut self, pane_id: &str, out: &mut Vec<Event>) {
        let Some(Ok(chat)) = self.panes.get_mut(pane_id).map(|f| &mut f.chat) else {
            return;
        };
        if let Err(err) = chat.earlier() {
            let path = chat.source.path.display();
            tracing::warn!("agent chat of pane {pane_id}: can't read {path}: {err}");
            out.push(Event::Failed {
                pane_id: pane_id.to_owned(),
                error: format!("Couldn't read this agent's earlier messages: {err}"),
            });
            // The next poll sends its items again: the chat comes back
            // with "Earlier messages" to press again.
            chat.sent = None;
            return;
        }
        chat.poll(pane_id, out);
    }

    /// Read what each chat grew by; look again for the chats not found.
    /// Read pane `pane_id`'s chat now, if it was found.
    fn read_now(&mut self, pane_id: &str, out: &mut Vec<Event>) {
        if let Some(Ok(chat)) = self.panes.get_mut(pane_id).map(|f| &mut f.chat) {
            chat.poll(pane_id, out);
        }
    }

    fn poll(&mut self, out: &mut Vec<Event>) {
        let mut again = Vec::new();
        for (pane_id, followed) in &mut self.panes {
            match &mut followed.chat {
                Ok(chat) => chat.poll(pane_id, out),
                Err(_) if followed.looked.elapsed() < FIND_AGAIN => {}
                Err(_) => again.push(pane_id.clone()),
            }
        }
        for pane_id in again {
            let agent = self.panes[&pane_id].agent.clone();
            let found = self.find(&agent);
            let Some(followed) = self.panes.get_mut(&pane_id) else {
                continue;
            };
            followed.looked = Instant::now();
            if found.as_ref().err() != followed.chat.as_ref().err() {
                followed.found(found, out);
            }
        }
    }

    /// The chat of the agent in `agent`'s pane: the one herdr names, else
    /// one found by the agent's folder and start time. Says when herdr's
    /// integration for the agent isn't installed, which is why it names
    /// none.
    fn find(&mut self, agent: &AgentInfo) -> Result<Source, String> {
        let found = source::find(agent, &self.homes);
        let Some(kind) = source::kind(agent).filter(|kind| *kind != AgentKind::Unknown) else {
            return found;
        };
        let target = kind.name();
        if source::named(agent) {
            crate::problems::clear_at(PROBLEMS, target);
            return found;
        }
        if found.as_ref().is_err_and(|why| why != source::NOT_FOUND) {
            return found;
        }
        let missing = self.integration(target).as_deref() == Some(NOT_INSTALLED);
        match missing {
            true => {
                crate::problems::report_at(PROBLEMS, target, source::not_installed(kind, target))
            }
            false => crate::problems::clear_at(PROBLEMS, target),
        }
        let run = match source::guessable(kind) {
            true => self.run_of(agent),
            false => None,
        };
        let claimed = self.claimed(&agent.pane_id);
        let guessed = run.map(|run| source::by_time(kind, &run, &self.homes, &claimed));
        match guessed {
            Some(Ok(Some(source))) => Ok(source),
            Some(Err(why)) => Err(why),
            _ if missing => Err(source::missing_integration(target)),
            _ => found,
        }
    }

    /// The ids of the chats of the panes other than `pane_id`.
    fn claimed(&self, pane_id: &str) -> HashSet<String> {
        (self.panes.iter())
            .filter(|(id, _)| *id != pane_id)
            .filter_map(|(_, followed)| followed.chat.as_ref().ok())
            .map(|chat| chat.source.id.clone())
            .collect()
    }

    /// The state herdr gives its integration `target`, from its list
    /// asked once an `INTEGRATIONS_AGAIN`. An integration no longer
    /// missing takes its problem with it.
    fn integration(&mut self, target: &str) -> Option<String> {
        let stale =
            (self.integrations.as_ref()).is_none_or(|(at, _)| at.elapsed() >= INTEGRATIONS_AGAIN);
        if stale && let Some(herdr) = self.herdr.as_mut() {
            let states = match herdr.call("integration.list", serde_json::json!({})) {
                Ok(reply) => integration_states(&reply),
                Err(err) => {
                    tracing::warn!("agent chats: herdr's integrations: {err}");
                    self.integrations
                        .take()
                        .map(|(_, states)| states)
                        .unwrap_or_default()
                }
            };
            for (target, _) in states.iter().filter(|(_, state)| *state != NOT_INSTALLED) {
                crate::problems::clear_at(PROBLEMS, target);
            }
            self.integrations = Some((Instant::now(), states));
        }
        self.integrations.as_ref()?.1.get(target).cloned()
    }

    /// When the agent in `agent`'s pane began and where it runs: herdr's
    /// foreground process there, as `/proc` says.
    fn run_of(&mut self, agent: &AgentInfo) -> Option<Run> {
        let info = (self.herdr.as_mut()?.process_info(&agent.pane_id))
            .map_err(|err| tracing::warn!("agent chat of pane {}: {err}", agent.pane_id))
            .ok()?;
        let processes = &info.foreground_processes;
        // The group's leader is what the shell started: the agent.
        let lead = (processes.iter())
            .find(|p| Some(p.pid) == info.foreground_process_group_id)
            .or(processes.first())?;
        if Some(lead.pid) == info.shell_pid {
            return None;
        }
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", lead.pid)).ok()?;
        let boot = std::fs::read_to_string("/proc/stat").ok()?;
        let cwd = (lead.cwd.clone())
            .or_else(|| agent.foreground_cwd.clone())
            .or_else(|| agent.cwd.clone())?;
        Some(Run {
            started: started_at(&stat, &boot, clock_ticks())?,
            cwd,
        })
    }
}

/// The states of herdr's integrations by target, out of its
/// `integration.list` reply; what it doesn't say is left out.
fn integration_states(reply: &Value) -> HashMap<String, String> {
    (reply["integrations"].as_array().into_iter().flatten())
        .filter_map(|it| {
            Some((
                it["target"].as_str()?.to_owned(),
                it["state"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// When a process began, in seconds since the epoch: out of its
/// `/proc/<pid>/stat` (`stat`, its start in clock ticks since boot) and
/// `/proc/stat` (`boot`, its `btime`).
fn started_at(stat: &str, boot: &str, ticks: u64) -> Option<u64> {
    // The name, in brackets, may hold spaces and brackets of its own.
    let fields = &stat[stat.rfind(')')? + 1..];
    // `starttime`, the 22nd field: the 20th after the name.
    let since_boot: u64 = fields.split_whitespace().nth(19)?.parse().ok()?;
    let btime: u64 = (boot.lines())
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(btime + since_boot / ticks.max(1))
}

/// The clock ticks a second `/proc` counts in.
fn clock_ticks() -> u64 {
    // SAFETY: sysconf reads a constant of the system.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as u64 } else { 100 }
}

impl Followed {
    /// Take chat `found`: read it, or say why there is none.
    fn found(&mut self, found: Result<Source, String>, out: &mut Vec<Event>) {
        let pane_id = self.agent.pane_id.clone();
        match found {
            Ok(source) => {
                let mut chat = Chat::new(source);
                chat.poll(&pane_id, out);
                self.chat = Ok(chat);
            }
            Err(why) => {
                let session = &self.agent.agent_session;
                tracing::warn!("agent chat of pane {pane_id}: {why} (session {session:?})");
                out.push(Event::Missing {
                    pane_id,
                    why: why.clone(),
                });
                self.chat = Err(why);
            }
        }
    }
}

mod db;

/// A transcript being read, line by line as it grows; or a database chat
/// (OpenCode's, Cursor's), read whole each time it changes.
struct Chat {
    source: Source,
    reader: Reader,
    /// A database chat's items, as last read.
    snapshot: Vec<Item>,
    /// A database chat's rows, as last read.
    kept: db::Kept,
    /// What the database and its WAL were (`stamp`) when last read.
    stamped: Option<[Option<Stamp>; 2]>,
    /// The file read (device and inode): another one is read from the start.
    file: Option<(u64, u64)>,
    /// How far it is read.
    offset: u64,
    /// Where its reading began: what is before is read on asking.
    from: u64,
    /// How much a read from the end, or further back, takes.
    chunk: u64,
    /// Claude Code's subagents' own transcripts, by file: how far each is
    /// read (see `read_subagents`).
    subagents: HashMap<PathBuf, u64>,
    /// The calls that started them, by file, as their meta files say.
    started_by: HashMap<PathBuf, String>,
    /// Whether `offset` is at a line's start: not when the first read began
    /// mid-line.
    whole: bool,
    /// The bytes just before `offset`, at most `PREV`.
    prev: Vec<u8>,
    /// The items last sent; None: send them next, changed or not.
    sent: Option<Arc<Vec<Item>>>,
    /// A read failed, and the user was told.
    failed: bool,
    /// How often it was read again from the start.
    resets: u32,
}

/// A file as it is: its inode, change time and length.
type Stamp = (u64, i64, i64, u64);

fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.ino(), meta.mtime(), meta.mtime_nsec(), meta.len()))
}

/// Whether chats of agent `kind` are kept in a database.
fn in_database(kind: AgentKind) -> bool {
    matches!(kind, AgentKind::OpenCode | AgentKind::Cursor)
}

impl Chat {
    fn new(source: Source) -> Self {
        Self {
            reader: Reader::new(source.agent),
            snapshot: Vec::new(),
            kept: db::Kept::default(),
            stamped: None,
            source,
            file: None,
            offset: 0,
            from: 0,
            chunk: CHAT_TAIL,
            subagents: HashMap::new(),
            started_by: HashMap::new(),
            whole: true,
            prev: Vec::new(),
            sent: None,
            failed: false,
            resets: 0,
        }
    }

    /// Read what it grew by, and send its items if they changed.
    fn poll(&mut self, pane_id: &str, out: &mut Vec<Event>) {
        let read = match in_database(self.source.agent) {
            true => self.read_rows(),
            false => {
                (self.read().and_then(|()| self.read_subagents())).map_err(|err| err.to_string())
            }
        };
        if let Err(err) = read {
            if !self.failed {
                let path = self.source.path.display();
                tracing::warn!("agent chat of pane {pane_id}: can't read {path}: {err}");
                out.push(Event::Failed {
                    pane_id: pane_id.to_owned(),
                    error: format!("Couldn't read this agent's chat: {err}"),
                });
                self.failed = true;
                self.sent = None;
            }
            return;
        }
        self.failed = false;
        let now = match in_database(self.source.agent) {
            true => &self.snapshot[..],
            false => self.reader.items(),
        };
        if self.sent.as_deref().map(Vec::as_slice) != Some(now) {
            let items = Arc::new(now.to_vec());
            self.sent = Some(items.clone());
            out.push(Event::Items {
                pane_id: pane_id.to_owned(),
                source: self.source.clone(),
                items,
                earlier: self.from > 0,
            });
        }
    }

    /// Read what changed in a database chat's rows if the database or its
    /// WAL changed since the last read.
    fn read_rows(&mut self) -> Result<(), String> {
        let mut wal = self.source.path.clone().into_os_string();
        wal.push("-wal");
        let now = [
            Some(stamp(&self.source.path).map_err(|err| err.to_string())?),
            stamp(Path::new(&wal)).ok(),
        ];
        if self.stamped == Some(now) {
            return Ok(());
        }
        let conn = crate::cursor_agents::open_read_only(&self.source.path)
            .and_then(|conn| conn.busy_timeout(Duration::from_secs(1)).map(|()| conn))
            .map_err(|err| err.to_string())?;
        (self.kept).read(self.source.agent, &self.source.id, &conn)?;
        let rows = self.kept.rows();
        self.snapshot = parse::snapshot(self.source.agent, rows.iter().map(|row| &**row));
        self.stamped = Some(now);
        self.resets += 1;
        Ok(())
    }

    /// Feed the reader the whole lines written since the last read. Another
    /// file, a shorter one or one whose bytes before the offset changed is
    /// read again from the start, with a fresh reader.
    fn read(&mut self) -> std::io::Result<()> {
        let mut file = File::open(&self.source.path)?;
        let meta = file.metadata()?;
        let (id, len) = ((meta.dev(), meta.ino()), meta.len());
        if self.file != Some(id) || len < self.offset {
            self.restart(id, len);
        }
        if len == self.offset {
            return Ok(());
        }
        let back = self.prev.len() as u64;
        file.seek(SeekFrom::Start(self.offset - back))?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(len - self.offset + back)
            .read_to_end(&mut bytes)?;
        if !bytes.starts_with(&self.prev) {
            self.restart(id, len);
            file.seek(SeekFrom::Start(self.offset))?;
            bytes.clear();
            (&mut file)
                .take(len - self.offset)
                .read_to_end(&mut bytes)?;
        }
        let back = self.prev.len();
        let new = &bytes[back..];
        // Begun mid-line: from the first whole one.
        let start = match self.whole {
            true => 0,
            false => match new.iter().position(|&b| b == b'\n') {
                Some(at) => {
                    self.whole = true;
                    at + 1
                }
                None => new.len(),
            },
        };
        // A last line still being written waits until it is whole.
        let end =
            (new[start..].iter().rposition(|&b| b == b'\n')).map_or(start, |at| start + at + 1);
        if end > start {
            self.reader.feed(&new[start..end]);
        }
        self.offset += end as u64;
        let before = &bytes[..back + end];
        self.prev = before[before.len().saturating_sub(PREV)..].to_vec();
        Ok(())
    }

    /// Read a `chunk` more of the transcript, before where its reading
    /// began: everything from there to where it is read, with a fresh
    /// reader, so the calls just before the old start meet their results.
    /// Items keep their ids, so what was unfolded stays so.
    fn earlier(&mut self) -> std::io::Result<()> {
        if self.from == 0 || in_database(self.source.agent) {
            return Ok(());
        }
        let mut file = File::open(&self.source.path)?;
        let meta = file.metadata()?;
        // Another file now: the next poll reads it from its end.
        if self.file != Some((meta.dev(), meta.ino())) || meta.len() < self.offset {
            return Ok(());
        }
        let from = self.from.saturating_sub(self.chunk);
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(self.offset - from)
            .read_to_end(&mut bytes)?;
        // Begun mid-line: from the first whole one.
        let start = match from {
            0 => 0,
            _ => (bytes.iter().position(|&b| b == b'\n')).map_or(bytes.len(), |at| at + 1),
        };
        let mut reader = Reader::new(self.source.agent);
        reader.feed(&bytes[start..]);
        self.reader = reader;
        self.subagents.clear();
        self.from = from;
        self.whole = true;
        self.sent = None;
        Ok(())
    }

    /// Feed the reader what Claude Code's subagents wrote since the last
    /// read, each in its own file beside the chat's
    /// (`<chat>/subagents/agent-<id>.jsonl`), the call that started it named
    /// in its `.meta.json`. Only those whose call was read: their work has a
    /// place to show. A file is read from its last `chunk` bytes.
    fn read_subagents(&mut self) -> std::io::Result<()> {
        if self.source.agent != AgentKind::Claude {
            return Ok(());
        }
        let dir = self.source.path.with_extension("").join("subagents");
        let files = match std::fs::read_dir(&dir) {
            Ok(files) => files,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        };
        for entry in files.flatten() {
            let path = entry.path();
            let Some(agent) = (path.file_name().and_then(|n| n.to_str()))
                .and_then(|n| n.strip_prefix("agent-")?.strip_suffix(".jsonl"))
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(call) = self.started_by(&path) else {
                continue;
            };
            if !self.reader.knows_call(&call) {
                continue;
            }
            if let Err(err) = self.read_subagent(&path, &agent, &call) {
                // One subagent unread leaves the chat as it is.
                tracing::warn!("agent chat: can't read {}: {err}", path.display());
            }
        }
        Ok(())
    }

    /// The call that started the subagent of file `path`, as its meta file
    /// says; asked again until it says.
    fn started_by(&mut self, path: &Path) -> Option<String> {
        if let Some(call) = self.started_by.get(path) {
            return Some(call.clone());
        }
        let meta = std::fs::read(path.with_extension("meta.json")).ok()?;
        let meta: Value = serde_json::from_slice(&meta).ok()?;
        let call = meta["toolUseId"].as_str()?.to_owned();
        self.started_by.insert(path.to_owned(), call.clone());
        Some(call)
    }

    /// The whole lines subagent `agent`'s file `path` grew by, fed.
    fn read_subagent(&mut self, path: &Path, agent: &str, call: &str) -> std::io::Result<()> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let offset = match self.subagents.get(path) {
            Some(&at) if at <= len => at,
            _ => len.saturating_sub(self.chunk),
        };
        if offset == len {
            self.subagents.insert(path.to_owned(), len);
            return Ok(());
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        (&mut file).take(len - offset).read_to_end(&mut bytes)?;
        // A last line still being written waits until it is whole.
        let end = bytes
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |at| at + 1);
        self.reader.feed_subagent(agent, call, &bytes[..end]);
        self.subagents.insert(path.to_owned(), offset + end as u64);
        Ok(())
    }

    /// Read file `id`, `len` bytes long, again from the start: its last
    /// `chunk` bytes.
    fn restart(&mut self, id: (u64, u64), len: u64) {
        self.reader = Reader::new(self.source.agent);
        self.subagents.clear();
        self.file = Some(id);
        self.offset = len.saturating_sub(self.chunk);
        self.from = self.offset;
        self.whole = self.offset == 0;
        self.prev.clear();
        self.sent = None;
        self.resets += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::chat::source::tests::agent;

    fn line(role: &str, text: &str) -> String {
        let id = format!("{role}-{text}");
        let message =
            serde_json::json!({ "role": role, "content": [{ "type": "text", "text": text }] });
        let line = serde_json::json!({ "type": role, "uuid": id, "message": message });
        format!("{line}\n")
    }

    struct Home {
        dir: PathBuf,
        chats: Chats,
        /// Held while it lives: following a chat reports and clears the
        /// agent's problem in gerdr's one list, which another test's
        /// following would clear under it.
        _alone: std::sync::MutexGuard<'static, ()>,
    }

    static ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl Home {
        fn new(name: &str) -> Self {
            let alone = ALONE.lock().unwrap_or_else(|e| e.into_inner());
            let dir =
                std::env::temp_dir().join(format!("gerdr-chat-tail-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join(".claude/projects/-work")).expect("dir");
            let chats = Chats::new(Homes::under(&dir), None);
            Self {
                dir,
                chats,
                _alone: alone,
            }
        }

        fn file(&self, id: &str) -> PathBuf {
            self.dir.join(format!(".claude/projects/-work/{id}.jsonl"))
        }

        fn append(&self, id: &str, text: &str) {
            let mut file = (std::fs::OpenOptions::new().create(true).append(true))
                .open(self.file(id))
                .expect("open");
            file.write_all(text.as_bytes()).expect("write");
        }

        fn follow(&mut self, id: &str) -> Vec<Event> {
            let mut out = Vec::new();
            self.chats.follow(agent("claude", "id", id), &mut out);
            out
        }

        fn poll(&mut self) -> Vec<Event> {
            let mut out = Vec::new();
            self.chats.poll(&mut out);
            out
        }

        fn chat(&self) -> &Chat {
            self.chats.panes["p1"].chat.as_ref().expect("a chat")
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn items_of(events: &[Event]) -> Vec<&str> {
        (events.iter())
            .filter_map(|event| match event {
                Event::Items { source, .. } => Some(source.id.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_chat_is_read_then_read_as_it_grows() {
        let mut home = Home::new("grow");
        let first = line("user", "hello");
        home.append("s1", &first);
        let events = home.follow("s1");
        assert_eq!(items_of(&events), ["s1"]);
        assert_eq!(home.chat().offset, first.len() as u64);
        assert!(home.poll().is_empty(), "nothing new");
        // A half-written line waits until it is whole.
        let second = line("assistant", "hi there");
        let (head, rest) = second.split_at(10);
        home.append("s1", head);
        home.poll();
        assert_eq!(home.chat().offset, first.len() as u64);
        home.append("s1", rest);
        home.poll();
        assert_eq!(home.chat().offset, (first.len() + second.len()) as u64);
        assert_eq!(home.chat().resets, 1, "read once from the start");
        // Asked again for the same chat: nothing new.
        assert!(home.follow("s1").is_empty());
    }

    #[test]
    fn a_shorter_or_rewritten_file_is_read_again() {
        let mut home = Home::new("reset");
        home.append("s1", &(line("user", "one") + &line("user", "two")));
        home.follow("s1");
        // Shorter.
        std::fs::write(home.file("s1"), line("user", "three")).expect("write");
        assert_eq!(items_of(&home.poll()), ["s1"]);
        assert_eq!(home.chat().resets, 2);
        assert_eq!(home.chat().offset, line("user", "three").len() as u64);
        // As long and more, but other bytes before the offset.
        let other = line("user", "fours") + &line("user", "five");
        std::fs::write(home.file("s1"), &other).expect("write");
        assert_eq!(items_of(&home.poll()), ["s1"]);
        assert_eq!(
            (home.chat().resets, home.chat().offset),
            (3, other.len() as u64)
        );
        // Another file in its place.
        let moved = home.dir.join("new.jsonl");
        std::fs::write(&moved, &other).expect("write");
        std::fs::rename(&moved, home.file("s1")).expect("rename");
        assert_eq!(items_of(&home.poll()), ["s1"]);
        assert_eq!(home.chat().resets, 4);
    }

    #[test]
    fn a_long_chat_is_read_from_its_end() {
        let mut home = Home::new("long");
        let filler = line("user", &"x".repeat(1000));
        let lines = (CHAT_TAIL as usize / filler.len()) + 10;
        home.append("s1", &filler.repeat(lines));
        home.follow("s1");
        let len = std::fs::metadata(home.file("s1")).unwrap().len();
        assert_eq!(home.chat().offset, len);
        assert!(home.chat().whole);
        home.append("s1", &line("user", "more"));
        home.poll();
        assert_eq!(home.chat().resets, 1);
    }

    #[test]
    fn earlier_messages_are_read_on_asking_back_to_the_start() {
        let mut home = Home::new("earlier");
        let said: Vec<String> = (0..40)
            .map(|i| line("user", &format!("message {i:02}")))
            .collect();
        home.append("s1", &said.concat());
        home.chats
            .follow(agent("claude", "id", "s1"), &mut Vec::new());
        // Read again with a small chunk: about the last ten lines.
        let chunk = (said[0].len() * 10) as u64;
        let pane = home.chats.panes.get_mut("p1").unwrap();
        let chat = pane.chat.as_mut().unwrap();
        chat.chunk = chunk;
        chat.file = None;
        let events = home.poll();
        let shown = |events: &[Event]| match events {
            [Event::Items { items, earlier, .. }] => (items.len(), *earlier),
            other => panic!("{other:?}"),
        };
        let (first, earlier) = shown(&events);
        assert!(earlier && (9..=10).contains(&first), "{first}");
        let mut out = Vec::new();
        home.chats.earlier("p1", &mut out);
        let (second, earlier) = shown(&out);
        assert!(earlier && second >= first + 9, "{second}");
        // Until the start: all of it, no more to ask for.
        for _ in 0..5 {
            out.clear();
            home.chats.earlier("p1", &mut out);
        }
        let all = home.chat().reader.items().to_vec();
        assert_eq!(all.len(), 40);
        assert_eq!(all[0].id(), "user-message 00.0");
        assert_eq!(home.chat().from, 0);
        out.clear();
        home.chats.earlier("p1", &mut out);
        assert!(out.is_empty(), "nothing more");
        // It still follows the end.
        home.append("s1", &line("assistant", "done"));
        match &home.poll()[..] {
            [Event::Items { items, earlier, .. }] => assert!(items.len() == 41 && !earlier),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_subagents_own_file_is_read_under_its_call() {
        let mut home = Home::new("subagents");
        let call = serde_json::json!({ "type": "assistant", "uuid": "a1", "message": {
            "id": "m1", "role": "assistant", "content": [{ "type": "tool_use", "id": "toolu_1",
            "name": "Agent", "input": { "description": "Read the docs", "prompt": "Read" } }] } });
        home.append("s1", &format!("{call}\n"));
        let dir = home.dir.join(".claude/projects/-work/s1/subagents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("agent-ag1.meta.json"),
            r#"{"toolUseId":"toolu_1"}"#,
        )
        .unwrap();
        // One whose call isn't in the chat: left out.
        std::fs::write(
            dir.join("agent-ag2.meta.json"),
            r#"{"toolUseId":"toolu_9"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("agent-ag2.jsonl"), line("user", "elsewhere")).unwrap();
        std::fs::write(dir.join("agent-ag1.jsonl"), line("user", "Read")).unwrap();
        home.follow("s1");
        let steps = |home: &Home| match home.chat().reader.items() {
            [Item::Subagent { items, .. }] => items.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!(steps(&home), 1);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("agent-ag1.jsonl"))
            .unwrap();
        file.write_all(line("assistant", "Reading").as_bytes())
            .unwrap();
        assert!(matches!(&home.poll()[..], [Event::Items { .. }]));
        assert_eq!(steps(&home), 2);
        assert!(home.poll().is_empty(), "nothing new");
    }

    #[test]
    fn a_new_session_switches_chats_and_stop_forgets() {
        let mut home = Home::new("switch");
        home.append("s1", &line("user", "one"));
        home.append("s2", &line("user", "two"));
        home.follow("s1");
        assert_eq!(items_of(&home.follow("s2")), ["s2"]);
        assert_eq!(home.chat().source.path, home.file("s2"));
        home.chats.stop("p1");
        assert!(home.chats.panes.is_empty());
        assert!(home.poll().is_empty());
    }

    #[test]
    fn a_chat_not_written_yet_is_found_once_it_is() {
        let mut home = Home::new("later");
        let events = home.follow("s1");
        assert!(matches!(&events[..], [Event::Missing { why, .. }] if why == source::NOT_YET));
        let ago = |home: &mut Home| {
            for followed in home.chats.panes.values_mut() {
                followed.looked -= FIND_AGAIN;
            }
        };
        ago(&mut home);
        assert!(home.poll().is_empty(), "told once");
        home.append("s1", &line("user", "one"));
        // Looked for again only once `FIND_AGAIN` is over.
        assert!(home.poll().is_empty());
        ago(&mut home);
        assert_eq!(items_of(&home.poll()), ["s1"]);
    }

    #[test]
    fn a_failed_read_is_told_once_and_tried_again() {
        let mut home = Home::new("failed");
        home.append("s1", &line("user", "one"));
        home.follow("s1");
        std::fs::remove_file(home.file("s1")).expect("remove");
        assert!(matches!(&home.poll()[..], [Event::Failed { .. }]));
        assert!(home.poll().is_empty(), "told once");
        home.append("s1", &line("user", "two"));
        assert_eq!(items_of(&home.poll()), ["s1"]);
    }

    #[test]
    fn the_thread_sends_and_ends_with_its_tail() {
        let home = Home::new("thread");
        home.append("s1", &line("user", "one"));
        let (out, events) = channel::channel();
        let (cmds, asked) = mpsc::channel();
        let homes = Homes::under(&home.dir);
        let thread = std::thread::spawn(move || run(&asked, &mut Chats::new(homes, None), &out));
        let tail = Tail { cmds };
        tail.follow(agent("claude", "id", "s1"));
        drop(tail);
        thread.join().expect("thread ends");
        let mut got = Vec::new();
        let mut events_loop =
            smithay::reexports::calloop::EventLoop::<Vec<String>>::try_new().unwrap();
        events_loop
            .handle()
            .insert_source(events, |event, _, got: &mut Vec<String>| {
                if let channel::Event::Msg(Event::Items { source, .. }) = event {
                    got.push(source.id);
                }
            })
            .unwrap();
        events_loop.dispatch(Duration::ZERO, &mut got).unwrap();
        assert_eq!(got, ["s1"]);
    }

    #[test]
    fn a_process_start_is_read_from_proc() {
        let stat = "4242 (a (b) c) S 1 4242 4242 0 -1 4194560 100 0 0 0 5 1 0 0 20 0 1 0 123456 \
                    1000 100 18446744073709551615";
        let boot = "cpu  1 2 3\nbtime 1790000000\nprocesses 9\n";
        assert_eq!(started_at(stat, boot, 100), Some(1_790_000_000 + 1234));
        assert_eq!(started_at("4242 (x) S 1", boot, 100), None);
        assert_eq!(started_at(stat, "cpu 1\n", 100), None);
        // This test's own process began a while ago, not in the future.
        let own = std::fs::read_to_string("/proc/self/stat").unwrap();
        let boot = std::fs::read_to_string("/proc/stat").unwrap();
        let started = started_at(&own, &boot, clock_ticks()).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            started <= now + 1 && now - started < 24 * 3600,
            "{started} {now}"
        );
    }

    #[test]
    fn herdr_integrations_read_leniently() {
        let reply = serde_json::json!({"type": "integration_list", "integrations": [
            {"target": "codex", "label": "codex", "available": false, "state": "not_installed"},
            {"target": "opencode", "state": "current"},
            {"target": "pi"},
            "junk",
        ]});
        let states = integration_states(&reply);
        assert_eq!(states.len(), 2);
        assert_eq!(states["codex"], NOT_INSTALLED);
        assert!(integration_states(&serde_json::json!({})).is_empty());
    }

    fn chat_problems(target: &str) -> Vec<String> {
        (crate::problems::list().into_iter())
            .filter(|p| p.source == PROBLEMS && p.text.contains(&format!(" {target} ")))
            .map(|p| p.text)
            .collect()
    }

    #[test]
    fn a_missing_integration_is_a_problem_until_a_session_shows() {
        let mut home = Home::new("integration");
        let states = HashMap::from([("codex".to_owned(), NOT_INSTALLED.to_owned())]);
        home.chats.integrations = Some((Instant::now(), states));
        let mut none = agent("codex", "id", "019a-1");
        none.agent_session = None;
        let mut out = Vec::new();
        home.chats.follow(none, &mut out);
        let why = source::missing_integration("codex");
        assert!(matches!(&out[..], [Event::Missing { why: w, .. }] if *w == why));
        let text = source::not_installed(AgentKind::Codex, "codex");
        assert_eq!(chat_problems("codex"), [text]);
        out.clear();
        home.chats.follow(agent("codex", "id", "019a-1"), &mut out);
        assert!(matches!(&out[..], [Event::Missing { why, .. }] if why == source::NOT_YET));
        assert!(chat_problems("codex").is_empty());
    }

    /// A Cursor `store.db` at `dir/.cursor/chats/f00/<id>/`, with `blobs`.
    fn cursor_store(dir: &Path, id: &str, blobs: &[&str]) -> PathBuf {
        let db = dir.join(format!(".cursor/chats/f00/{id}/store.db"));
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB);")
            .unwrap();
        add_blobs(&db, blobs);
        db
    }

    fn add_blobs(db: &Path, blobs: &[&str]) {
        let conn = rusqlite::Connection::open(db).unwrap();
        for blob in blobs {
            let id: i64 = conn
                .query_row("SELECT count(*) FROM blobs", [], |r| r.get(0))
                .unwrap();
            conn.execute(
                "INSERT INTO blobs VALUES (?1, ?2)",
                rusqlite::params![id.to_string(), blob.as_bytes()],
            )
            .unwrap();
        }
    }

    #[test]
    fn a_database_chat_is_read_again_only_when_it_changed() {
        let mut home = Home::new("snapshot");
        let db = cursor_store(&home.dir, "c-1", &[r#"{"role":"user","content":"go"}"#]);
        home.chats
            .follow(agent("cursor", "id", "c-1"), &mut Vec::new());
        assert_eq!(home.chat().source.path, db);
        assert_eq!(home.chat().resets, 1);
        home.poll();
        home.poll();
        assert_eq!(home.chat().resets, 1, "unchanged: not read again");
        add_blobs(&db, &[r#"{"role":"assistant","content":"done"}"#, "\u{1}x"]);
        home.poll();
        assert_eq!(home.chat().resets, 2);
        let rows = home.chat().kept.rows();
        let roles: Vec<&str> = rows.iter().filter_map(|r| r["role"].as_str()).collect();
        assert_eq!(roles, ["user", "assistant"]);
        // Gone: told once, then read again once it is back.
        std::fs::remove_file(&db).unwrap();
        assert!(matches!(&home.poll()[..], [Event::Failed { .. }]));
        assert!(home.poll().is_empty(), "told once");
        cursor_store(&home.dir, "c-1", &[]);
        assert!(matches!(&home.poll()[..], [Event::Items { .. }]));
    }

    /// A herdr on a socket of its own answering `integration.list` and
    /// `pane.process_info` with process `pid` in `cwd`, `calls` times.
    fn fake_herdr(name: &str, pid: u32, cwd: &Path, calls: usize) -> PathBuf {
        use std::{io::BufRead as _, os::unix::net::UnixListener};
        let socket = std::env::temp_dir().join(format!("gerdr-tail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let cwd = cwd.to_path_buf();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(calls) {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                std::io::BufReader::new(&stream)
                    .read_line(&mut line)
                    .unwrap();
                let asked: Value = serde_json::from_str(&line).unwrap();
                let result = match asked["method"].as_str() {
                    Some("integration.list") => serde_json::json!({"integrations": [
                        {"target": "cursor", "state": "not_installed"},
                    ]}),
                    _ => serde_json::json!({"process_info": {
                        "shell_pid": 1, "foreground_process_group_id": pid,
                        "foreground_processes": [{"pid": pid, "name": "cursor-agent", "cwd": cwd}],
                    }}),
                };
                let reply = serde_json::json!({"id": asked["id"], "result": result});
                writeln!(stream, "{reply}").unwrap();
            }
        });
        socket
    }

    #[test]
    fn a_pane_herdr_names_no_chat_of_gets_a_guess_by_folder_and_time() {
        let mut home = Home::new("guess");
        let transcript = home
            .dir
            .join(".cursor/projects/work-app/agent-transcripts/c-1");
        std::fs::create_dir_all(&transcript).unwrap();
        cursor_store(&home.dir, "c-1", &[]);
        // A process begun just now stands for the agent.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let socket = fake_herdr("guess", child.id(), Path::new("/work/app"), 2);
        home.chats.herdr = Some(Herdr::on_socket(socket.clone()));
        let mut none = agent("cursor", "id", "c-1");
        none.agent_session = None;
        home.chats.follow(none, &mut Vec::new());
        child.kill().ok();
        child.wait().ok();
        let source = &home.chat().source;
        assert_eq!((source.id.as_str(), source.guessed), ("c-1", true));
        assert_eq!(
            chat_problems("cursor"),
            [source::not_installed(AgentKind::Cursor, "cursor")]
        );
        let _ = std::fs::remove_file(&socket);
    }
}
