//! T3 Code's threads as agents (see `agents`). T3 Code (t3.codes) runs
//! coding agents (Claude, Codex, Cursor, OpenCode, …) headless in a server
//! of its own and shows them in a web app that server serves; gerdr shows
//! that app in a web pane like any other page. The server keeps its
//! threads in a SQLite database under its home (`~/.t3`, or
//! `$T3CODE_HOME`), which a thread reads every `DB_INTERVAL`, read only:
//! each thread's latest run says whether it works, waits for the user or
//! finished since the user last looked at it (`lastVisitedAt`, which T3
//! Code sets itself when its app shows the thread). T3 Code's app marks
//! the thread it shows visited the moment it finishes, seen or not, so a
//! thread left open in a pane never reads as done: gerdr holds done those
//! it saw finish out of the user's sight until the user looks at them, in
//! gerdr or in an app of T3 Code's own (`opens.openedAt`, which T3 Code
//! sets as a thread is opened), across restarts (`Store::t3_finished`, see
//! `hold_finished`).
//! Where the server runs is in `userdata/server-runtime.json`.
//!
//! The database is T3 Code's own, not an interface, and changes as T3 Code
//! likes. While it can't be read (its tables changed: the query no longer
//! prepares), gerdr asks the server instead, by `t3 thread list --json`
//! for each workspace's folder (`Source::Cli`): slower, and with less in
//! it (working, waiting or idle; no side chats, no news since the user
//! looked), but T3 Code's own interface. The database is tried again
//! every `RETRY_DATABASE`. Which version of T3 Code runs, and moving it
//! only to versions gerdr reads, is `t3_update`'s.
//!
//! Side chats and subagents show as lines on their parent's card. A card
//! closed is hidden from gerdr until its thread changes again (see
//! `Hidden`); one closed while it runs, until that run is over.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::herdr::AgentStatus;

/// How often the database is read.
pub const DB_INTERVAL: Duration = Duration::from_secs(1);
/// How long a missing database is waited for before it is looked for
/// again.
const DB_RETRY: Duration = Duration::from_secs(5);
/// How often where the server runs is read again.
const SERVER_INTERVAL: Duration = Duration::from_secs(5);
/// How many idle threads a workspace shows at most.
pub const HISTORY_CARDS: usize = 5;
/// How often the database is tried again while gerdr asks the server.
const RETRY_DATABASE: Duration = Duration::from_secs(30);
/// How often the server is asked at least, while the database can't be
/// read.
const CLI_INTERVAL: Duration = Duration::from_secs(30);
/// How soon after the last ask the server is asked again when the
/// database changed.
const CLI_SOON: Duration = Duration::from_secs(5);
/// How long `t3 thread list` may take.
const CLI_TIME: Duration = Duration::from_secs(20);
/// How many idle threads of a folder's project the server's list gives.
const CLI_IDLE: usize = 20;
/// The database, under T3 Code's home.
pub const DB: &str = "userdata/statev2.sqlite";
/// Where the server says it runs, under T3 Code's home.
const RUNTIME: &str = "userdata/server-runtime.json";
/// The server's environment's id, under T3 Code's home.
const ENVIRONMENT: &str = "userdata/environment-id";
/// What `t3 pair` prints before the link that pairs a browser.
const PAIRING_URL: &str = "Pairing URL:";
/// How long before a web pane back at T3 Code's sign-in page is sent
/// another pairing link.
const PAIR_AGAIN: Duration = Duration::from_secs(30);
/// How many pairing links a web pane is sent in a row, back at the
/// sign-in page each time, before gerdr gives up and says so.
const PAIR_TRIES: u32 = 3;
/// How long a new thread asked of a workspace without T3 Code's app open
/// waits for its pane to show the app (signing in included).
const NEW_THREAD_WAIT: Duration = Duration::from_secs(120);
/// The run states of a thread at work.
const WORKING: [&str; 4] = ["preparing", "queued", "starting", "running"];
/// The run state of a thread waiting for the user.
const WAITING: &str = "waiting";

/// T3 Code's home: `$T3CODE_HOME`, else `~/.t3`; `None` when it isn't
/// there (T3 Code never ran).
pub fn home() -> Option<PathBuf> {
    let home = std::env::var_os("T3CODE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| crate::home_dir().map(|home| home.join(".t3")))?;
    home.join("userdata").is_dir().then_some(home)
}

/// A new thread waiting for its workspace's pane to show T3 Code's app.
#[derive(Clone, Debug)]
struct NewThread {
    /// The workspace's folder.
    dir: PathBuf,
    /// What its draft says (see `Page::Draft`).
    prompt: Option<String>,
    at: Instant,
}

/// `secs` since the epoch as T3 Code writes its times (see
/// `ThreadRow::updated`), down to the second: a time within that second
/// sorts after it as text.
fn iso_utc(secs: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let days = (secs / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let rest = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Whether `a` and `b` are the same folder, however they are written.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || (a.file_name() == b.file_name()
            && matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b))
}

/// The thread that came of a stage's draft in folder `dir`, opened at `since`
/// (unix seconds): the first asked to run in that folder's project since,
/// that isn't in `taken`. Nothing else tells it from a chat the user
/// started there meanwhile: T3 Code gives a draft's thread no mark of it.
fn drafted_thread<'a>(
    rows: &'a [ThreadRow],
    dir: &Path,
    since: u64,
    taken: &[&str],
) -> Option<&'a ThreadRow> {
    let since = iso_utc(since);
    rows.iter()
        .filter(|r| !r.child && r.parent.is_none() && !taken.contains(&r.id.as_str()))
        .filter(|r| r.project.as_deref().is_some_and(|p| same_dir(p, dir)))
        .filter(|r| {
            r.first_run
                .as_deref()
                .is_some_and(|at| at >= since.as_str())
        })
        .min_by(|a, b| a.first_run.cmp(&b.first_run))
}

/// Where in T3 Code's app to show (see `Gerdr::show_t3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page<'a> {
    /// Wherever the app is, or its home page in a new tab.
    App,
    /// A thread, by id.
    Thread(&'a str),
    /// A new thread, in the workspace's project.
    New,
    /// A draft saying this prompt, in the project at the workspace's folder
    /// and no other, for the user to pick a model and send.
    Draft(&'a str),
}

/// Where T3 Code's server runs, as it says in its home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    /// Its address, e.g. `http://127.0.0.1:3773`, without a slash at the end.
    pub origin: String,
    pub pid: u32,
    pub version: Option<String>,
    /// Its environment's id, the first part of a thread's link.
    pub environment: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Runtime {
    origin: String,
    pid: u32,
    #[serde(default)]
    server_version: Option<String>,
}

impl Server {
    /// What T3 Code's home `home` says of its server; `None` if it never
    /// said.
    pub fn read(home: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(home.join(RUNTIME)).ok()?;
        let runtime: Runtime = serde_json::from_str(&text).ok()?;
        let environment = std::fs::read_to_string(home.join(ENVIRONMENT))
            .ok()
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());
        Some(Self {
            origin: runtime.origin.trim_end_matches('/').to_owned(),
            pid: runtime.pid,
            version: runtime.server_version,
            environment,
        })
    }

    /// Whether it still runs: its process is there.
    pub fn alive(&self) -> bool {
        Self::alive_pid(self.pid)
    }

    /// Whether process `pid` is there.
    pub fn alive_pid(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    /// Its port.
    pub fn port(&self) -> Option<u16> {
        site(&self.origin).map(|(_, port)| port)
    }

    /// The link of thread `id` in its app, at `origin` (its own, or the
    /// one a pane shows it at: the browser's sign-in is the host's).
    pub fn thread_link(&self, origin: &str, id: &str) -> String {
        match &self.environment {
            Some(env) => format!("{origin}/{}/{}", encode(env), encode(id)),
            None => format!("{origin}/"),
        }
    }
}

/// The host and port of web link `url`, the default port for its scheme
/// unless it says.
pub fn site(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, port.parse().ok()?),
        _ if scheme.eq_ignore_ascii_case("https") => (authority, 443),
        _ => (authority, 80),
    };
    Some((host.to_ascii_lowercase(), port))
}

/// The `scheme://host:port` of web link `url`.
pub fn origin_of(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[..scheme.len() + 3 + end])
}

/// Whether web link `url` is T3 Code's app of `server`: on this machine,
/// at its port. A workspace's app is at a host of its own under
/// `localhost` (see `app_host`).
pub fn is_app(url: &str, server: &Server) -> bool {
    let Some((host, port)) = site(url) else {
        return false;
    };
    let local = matches!(
        host.as_str(),
        "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0"
    ) || host.ends_with(".localhost");
    local && Some(port) == server.port()
}

/// The host a workspace in folder `dir` shows T3 Code's app at: a name of
/// its own under `localhost`, which browsers take to this machine. Each
/// host has its own storage and sign-in in the browser, so each
/// workspace's app keeps its own project filter (see `Gerdr::scope_t3`);
/// workspaces in one folder share one. The name is the folder's, then a
/// short hash of its whole path, so two folders of one name (`/w/a/app`,
/// `/w/b/app`) have hosts of their own: one DNS label, 59 bytes at most.
pub fn app_host(dir: &Path) -> String {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut label = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            label.push(c);
        } else if !label.is_empty() && !label.ends_with('-') {
            label.push('-');
        }
    }
    let label = label.trim_end_matches('-');
    let label = label.get(..label.len().min(50)).unwrap_or(label);
    let label = label.trim_end_matches('-');
    let label = if label.is_empty() { "root" } else { label };
    format!("{label}-{:08x}.localhost", path_hash(dir))
}

/// A hash of `dir`'s path that stays the same from one run, and Rust
/// version, to the next (FNV-1a, folded to 32 bits): a host named by it
/// keeps its browser storage.
fn path_hash(dir: &Path) -> u32 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in dir.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    (hash ^ (hash >> 32)) as u32
}

/// `link` at `origin` in place of its own.
fn at_origin(link: &str, origin: &str) -> String {
    match origin_of(link) {
        Some(own) => format!("{origin}{}", &link[own.len()..]),
        None => link.to_owned(),
    }
}

/// The thread a link of T3 Code's app shows, if it shows one: its path is
/// `/<environment>/<thread>`.
pub fn thread_of_link(url: &str) -> Option<String> {
    let origin = origin_of(url)?;
    let path = url[origin.len()..].split(['?', '#']).next()?;
    let mut parts = path.trim_matches('/').split('/');
    let (_env, thread) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || thread.is_empty() {
        return None;
    }
    Some(decode(thread))
}

/// `text` with every byte but the unreserved ones percent-encoded, as a
/// path's segment.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `text` with its percent-encoded bytes decoded.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A thread as the database has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadRow {
    pub id: String,
    pub title: String,
    /// When it last changed, as T3 Code writes it (ISO 8601, UTC): such
    /// times sort as text.
    pub updated: String,
    /// The folder of its project.
    pub project: Option<PathBuf>,
    /// Its worktree, when it works in one of its own.
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    /// The thread it was started from, for a side chat or a subagent.
    pub parent: Option<String>,
    /// It shows on its parent's card: a side chat or a subagent (a fork
    /// stands on its own).
    pub child: bool,
    /// The agent its latest run ran (T3 Code's driver, e.g.
    /// `claudeAgent`), else its provider's instance.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Its latest run's state and when that ended.
    pub run: Option<String>,
    pub completed: Option<String>,
    /// When the user last looked at it in T3 Code's app.
    pub visited: Option<String>,
    /// How many of its requests (approvals, questions) wait for the user.
    pub pending: i64,
    /// When it was last opened in T3 Code's app, in milliseconds since the
    /// epoch: a pane showing it already doesn't, as it finishes.
    pub opened: Option<i64>,
    /// When its first run was asked for, as `updated` is written; when it
    /// was made, from the server's list.
    pub first_run: Option<String>,
}

impl ThreadRow {
    /// What the row says it is doing.
    pub fn status(&self) -> AgentStatus {
        let run = self.run.as_deref();
        if self.pending > 0 || run == Some(WAITING) {
            AgentStatus::Blocked
        } else if run.is_some_and(|r| WORKING.contains(&r)) {
            AgentStatus::Working
        } else if self
            .completed
            .as_deref()
            .is_some_and(|done| self.visited.as_deref().is_none_or(|seen| done > seen))
        {
            AgentStatus::Done
        } else {
            AgentStatus::Idle
        }
    }

    /// The folders it works in, the likeliest first.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.worktree.iter().chain(&self.project).cloned().collect()
    }

    /// The agent it runs as herdr names agents (`claude`, `codex`, …).
    pub fn agent(&self) -> String {
        let provider = self.provider.as_deref().unwrap_or("agent");
        let lower = provider.to_ascii_lowercase();
        match lower.strip_suffix("agent") {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => lower,
        }
    }
}

/// A thread shown as a card: its row, what it is doing (its children's
/// work counting as its own) and its children that are not idle.
#[derive(Clone, Debug, PartialEq)]
pub struct T3Thread {
    pub row: ThreadRow,
    pub status: AgentStatus,
    pub children: Vec<(ThreadRow, AgentStatus)>,
}

/// The cards `rows` make: one per thread but the children, which go on
/// their topmost parent's. With the server gone (`alive` false) nothing
/// works or waits any more. The most recently changed first.
pub fn threads(rows: &[ThreadRow], alive: bool) -> Vec<T3Thread> {
    let status = |row: &ThreadRow| match row.status() {
        AgentStatus::Working | AgentStatus::Blocked if !alive => AgentStatus::Idle,
        status => status,
    };
    let by_id: BTreeMap<&str, &ThreadRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    // A child's card is its topmost parent's that is read.
    let root_of = |row: &ThreadRow| {
        let mut at = row;
        for _ in 0..rows.len() {
            let parent = at.parent.as_deref().filter(|_| at.child);
            match parent.and_then(|p| by_id.get(p)) {
                Some(parent) => at = parent,
                None => break,
            }
        }
        at.id.clone()
    };
    let mut cards: Vec<T3Thread> = rows
        .iter()
        .filter(|row| root_of(row) == row.id)
        .map(|row| T3Thread {
            row: row.clone(),
            status: status(row),
            children: Vec::new(),
        })
        .collect();
    for row in rows.iter().filter(|row| root_of(row) != row.id) {
        let root = root_of(row);
        let child_status = status(row);
        let Some(card) = cards.iter_mut().find(|c| c.row.id == root) else {
            continue;
        };
        if child_status != AgentStatus::Idle {
            card.children.push((row.clone(), child_status));
        }
        if crate::chrome::urgency(child_status) > crate::chrome::urgency(card.status)
            && matches!(child_status, AgentStatus::Working | AgentStatus::Blocked)
        {
            card.status = child_status;
        }
    }
    cards.sort_by(|a, b| b.row.updated.cmp(&a.row.updated));
    cards
}

/// The T3 Code threads held done, each with when it was last opened in
/// T3 Code's app as gerdr last knew it (`ThreadRow::opened`, 0 for
/// never); `None` until first read, for those saved before gerdr kept it.
pub type Held = BTreeMap<String, Option<i64>>;

/// `Held` as saved; once a list of the threads alone.
pub fn de_held<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Held, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Ids(Vec<String>),
        Held(Held),
    }
    Ok(match Saved::deserialize(d)? {
        Saved::Ids(ids) => ids.into_iter().map(|id| (id, None)).collect(),
        Saved::Held(held) => held,
    })
}

/// What gerdr sees of T3 Code's threads, for `hold_finished`.
struct Sight<'a> {
    /// Whether the server runs.
    alive: bool,
    /// The thread the user looks at in gerdr.
    seen: Option<&'a str>,
    /// The threads shown in this gerdr's workspaces.
    mine: &'a HashSet<String>,
    /// The threads gerdr's panes show, focused or not.
    in_panes: &'a HashSet<String>,
}

/// Hold done the cards of `threads` that `finished` holds, given the cards
/// were `before`: a card of `mine` that stopped working while the server
/// runs and the user didn't look is held from now. One at work again,
/// looked at in gerdr, or opened in T3 Code's app outside gerdr's panes
/// (a pane opening it, as gerdr starts or a page reloads, is no look) is
/// let go, and so is one no longer read. Whether `finished` changed.
fn hold_finished(
    finished: &mut Held,
    before: &[T3Thread],
    threads: &mut [T3Thread],
    sight: &Sight,
) -> bool {
    use AgentStatus::{Blocked, Done, Idle, Working};
    let held = finished.clone();
    let was = |id: &str| before.iter().find(|t| t.row.id == id).map(|t| t.status);
    for thread in threads.iter_mut() {
        let id = thread.row.id.as_str();
        let opened = thread.row.opened;
        if sight.seen == Some(id) || matches!(thread.status, Working | Blocked) {
            finished.remove(id);
        } else if let Some(known) = finished.get_mut(id) {
            if let Some(now) = opened.filter(|now| Some(*now) != *known) {
                if known.is_none() || sight.in_panes.contains(id) {
                    *known = Some(now);
                } else {
                    finished.remove(id);
                }
            }
        } else if sight.alive
            && sight.mine.contains(id)
            && matches!(was(id), Some(Working | Blocked))
        {
            finished.insert(id.to_owned(), Some(opened.unwrap_or(0)));
        }
        if thread.status == Idle && finished.contains_key(id) {
            thread.status = Done;
        }
    }
    // Nothing read yet (as gerdr starts) forgets nothing.
    if !threads.is_empty() {
        finished.retain(|id, _| threads.iter().any(|t| t.row.id == *id));
    }
    *finished != held
}

/// The threads gerdr hides, in `state.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hidden {
    /// The threads deleted from gerdr, each with when it had last changed
    /// then: one that changes again is back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deleted: BTreeMap<String, String>,
    /// The threads closed while they ran: hidden, and deleted once their
    /// run is over (see `settle`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub closed: BTreeSet<String>,
}

impl Hidden {
    pub fn is_empty(&self) -> bool {
        self.deleted.is_empty() && self.closed.is_empty()
    }

    /// Whether `thread` is hidden.
    pub fn hides(&self, thread: &T3Thread) -> bool {
        let row = &thread.row;
        self.closed.contains(&row.id)
            || self
                .deleted
                .get(&row.id)
                .is_some_and(|at| *at >= row.updated)
    }

    /// Hide `thread` from gerdr: until it changes again, or, while it
    /// runs, until its run is over and it changes after that.
    pub fn hide(&mut self, thread: &T3Thread) {
        match thread.status {
            AgentStatus::Working | AgentStatus::Blocked => {
                self.closed.insert(thread.row.id.clone());
            }
            _ => {
                self.closed.remove(&thread.row.id);
                self.deleted
                    .insert(thread.row.id.clone(), thread.row.updated.clone());
            }
        }
    }

    /// Take in the cards of a fresh read: those closed whose run is over
    /// are deleted as it left them, and threads no longer read or changed
    /// since are forgotten. Whether anything changed.
    pub fn settle(&mut self, threads: &[T3Thread]) -> bool {
        let before = self.clone();
        for thread in threads {
            let over = !matches!(thread.status, AgentStatus::Working | AgentStatus::Blocked);
            if over && self.closed.remove(&thread.row.id) {
                self.deleted
                    .insert(thread.row.id.clone(), thread.row.updated.clone());
            }
        }
        let read = |id: &str| threads.iter().find(|t| t.row.id == id);
        self.closed.retain(|id| read(id).is_some());
        self.deleted
            .retain(|id, at| read(id).is_some_and(|t| t.row.updated <= *at));
        *self != before
    }
}

/// The threads in the database at `conn` worth a card: those changed in
/// the last day, at work, waiting, done since the user last looked, or in
/// `held` (held done by gerdr, however old: see `hold_finished`); none
/// archived, deleted or settled.
pub fn read_rows(conn: &rusqlite::Connection, held: &[String]) -> rusqlite::Result<Vec<ThreadRow>> {
    let mut query = conn.prepare_cached(
        "SELECT t.thread_id, t.title, t.updated_at,
            p.workspace_root,
            json_extract(t.payload_json, '$.worktreePath'),
            json_extract(t.payload_json, '$.branch'),
            json_extract(t.payload_json, '$.lineage.parentThreadId'),
            json_extract(t.payload_json, '$.lineage.relationshipToParent'),
            json_extract(t.payload_json, '$.lineage.sideChat'),
            coalesce(r.provider, t.provider_instance_id),
            json_extract(t.payload_json, '$.modelSelection.model'),
            r.status, r.completed_at,
            json_extract(t.payload_json, '$.lastVisitedAt'),
            (SELECT count(*) FROM orchestration_v2_projection_runtime_requests q
              WHERE q.thread_id = t.thread_id AND q.status = 'pending'),
            json_extract(t.payload_json, '$.opens.openedAt'),
            (SELECT min(requested_at) FROM orchestration_v2_projection_runs
              WHERE thread_id = t.thread_id)
        FROM orchestration_v2_projection_threads t
        LEFT JOIN projection_projects p ON p.project_id = t.project_id
        LEFT JOIN orchestration_v2_projection_runs r ON r.run_id = (
            SELECT run_id FROM orchestration_v2_projection_runs
            WHERE thread_id = t.thread_id ORDER BY ordinal DESC LIMIT 1)
        WHERE t.archived_at IS NULL AND t.deleted_at IS NULL
          AND json_extract(t.payload_json, '$.settledAt') IS NULL
          AND (t.updated_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day')
               OR r.status IN ('preparing', 'queued', 'starting', 'running', 'waiting')
               OR r.completed_at > coalesce(json_extract(t.payload_json, '$.lastVisitedAt'), '')
               OR t.thread_id IN (SELECT value FROM json_each(?1)))",
    )?;
    let held = serde_json::to_string(held).unwrap_or_else(|_| "[]".to_owned());
    let rows = query.query_map([held], |r| {
        let path = |i: usize| r.get::<_, Option<String>>(i).map(|p| p.map(PathBuf::from));
        let relation: Option<String> = r.get(7)?;
        let side: Option<i64> = r.get(8)?;
        Ok(ThreadRow {
            id: r.get(0)?,
            title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            updated: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            project: path(3)?,
            worktree: path(4)?,
            branch: r.get(5)?,
            parent: r.get(6)?,
            child: relation.as_deref() == Some("subagent") || side.unwrap_or(0) != 0,
            provider: r.get(9)?,
            model: r.get(10)?,
            run: r.get(11)?,
            completed: r.get(12)?,
            visited: r.get(13)?,
            pending: r.get(14)?,
            opened: r.get(15)?,
            first_run: r.get(16)?,
        })
    })?;
    rows.collect()
}

/// Where gerdr reads T3 Code's threads from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// The database, in full.
    #[default]
    Database,
    /// The server's `t3 thread list`, as the database can't be read: why.
    Cli(String),
}

/// Where the problem of reading T3 Code in part comes from, for
/// `problems::set`.
const T3_PROBLEMS: &str = "t3";

/// Where the problem of a project filter the app didn't take is kept (see
/// `Gerdr::t3_unscoped`).
const FILTER_PROBLEMS: &str = "t3 code";

/// What the problems list says while gerdr reads T3 Code from `source`:
/// nothing while it reads the database.
fn partial_problem(source: &Source) -> Vec<String> {
    match source {
        Source::Database => Vec::new(),
        Source::Cli(why) => vec![format!(
            "gerdr can't read T3 Code's database ({why}), so its threads show basic status \
             only. \u{201c}Update T3 Code\u{201d} in the launcher can switch to a version gerdr \
             fully reads"
        )],
    }
}

/// What the watcher read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub rows: Vec<ThreadRow>,
    pub source: Source,
    /// The migration the database is at, if it says.
    pub migration: Option<i64>,
}

/// Read the threads of T3 Code at home `home` every `DB_INTERVAL`, on a
/// thread of its own, and hand them to `send` whenever they changed: from
/// the database, else from the server for the folders in `dirs` (see
/// `Source`). Stops once `send` returns false.
pub fn watch_db(
    home: PathBuf,
    dirs: Arc<Mutex<Vec<PathBuf>>>,
    held: Arc<Mutex<Vec<String>>>,
    mut send: impl FnMut(Snapshot) -> bool + Send + 'static,
) {
    std::thread::spawn(move || {
        let db = home.join(DB);
        let mut conn: Option<rusqlite::Connection> = None;
        let mut version: Option<i64> = None;
        let mut last: Option<Snapshot> = None;
        // Why the database isn't read, while it isn't.
        let mut broken: Option<String> = None;
        let mut asked: Option<Instant> = None;
        let mut tried = Instant::now();
        let held = move || held.lock().map(|h| h.clone()).unwrap_or_default();
        let mut emit = |snapshot: Snapshot| {
            if last.as_ref() == Some(&snapshot) {
                return true;
            }
            last = Some(snapshot.clone());
            send(snapshot)
        };
        loop {
            if conn.is_none() && db.exists() {
                conn = open_read_only(&db)
                    .inspect_err(|err| tracing::debug!("t3 code's database: {err}"))
                    .ok();
                version = None;
            }
            let now_version = conn
                .as_ref()
                .and_then(|c| c.query_row("PRAGMA data_version", [], |r| r.get(0)).ok());
            let changed = now_version.is_none() || now_version != version;
            if broken.is_none() {
                match &conn {
                    None if db.exists() => {}
                    None => broken = Some(format!("no database at {}", db.display())),
                    Some(_) if !changed => {}
                    Some(c) => match read_rows(c, &held()) {
                        Ok(rows) => {
                            version = now_version;
                            let migration = migration(c);
                            let source = Source::Database;
                            if !emit(Snapshot {
                                rows,
                                source,
                                migration,
                            }) {
                                return;
                            }
                        }
                        Err(err) if busy(&err) => {}
                        Err(err) => {
                            // Its tables changed, likely: said once.
                            tracing::warn!("t3 code's database, {}: {err}", db.display());
                            broken = Some(err.to_string());
                            asked = None;
                            tried = Instant::now();
                        }
                    },
                }
            }
            if let Some(why) = &broken {
                // Back to the database once it reads again.
                if tried.elapsed() >= RETRY_DATABASE {
                    tried = Instant::now();
                    conn = db.exists().then(|| open_read_only(&db).ok()).flatten();
                    if let Some(c) = &conn
                        && let Ok(rows) = read_rows(c, &held())
                    {
                        tracing::info!("t3 code's database reads again");
                        broken = None;
                        version = None;
                        let migration = migration(c);
                        let source = Source::Database;
                        if !emit(Snapshot {
                            rows,
                            source,
                            migration,
                        }) {
                            return;
                        }
                        continue;
                    }
                }
                let due = asked.is_none_or(|at| {
                    at.elapsed() >= CLI_INTERVAL || (changed && at.elapsed() >= CLI_SOON)
                });
                if due {
                    asked = Some(Instant::now());
                    version = now_version;
                    let dirs = dirs.lock().map(|d| d.clone()).unwrap_or_default();
                    let rows = cli_rows(&home, &dirs);
                    let migration = conn.as_ref().and_then(migration);
                    let source = Source::Cli(why.clone());
                    if !emit(Snapshot {
                        rows,
                        source,
                        migration,
                    }) {
                        return;
                    }
                }
            }
            std::thread::sleep(if conn.is_none() && broken.is_none() {
                DB_RETRY
            } else {
                DB_INTERVAL
            });
        }
    });
}

/// Whether `err` is only T3 Code writing meanwhile.
fn busy(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

/// The latest migration of the database at `conn`, if it says.
pub fn migration(conn: &rusqlite::Connection) -> Option<i64> {
    conn.query_row(
        "SELECT max(migration_id) FROM effect_sql_migrations",
        [],
        |r| r.get(0),
    )
    .ok()
    .flatten()
}

/// The threads of the projects of folders `dirs`, as the running server of
/// T3 Code at home `home` lists them; none without a server.
fn cli_rows(home: &Path, dirs: &[PathBuf]) -> Vec<ThreadRow> {
    let Some(server) = Server::read(home).filter(Server::alive) else {
        return Vec::new();
    };
    let Some(binary) = t3_binary(home, &server) else {
        return Vec::new();
    };
    let mut rows: Vec<ThreadRow> = Vec::new();
    for dir in dirs {
        let mut list = std::process::Command::new(&binary);
        list.args(["thread", "list", "--json", "--project"])
            .arg(dir)
            .arg("--base-dir")
            .arg(home)
            .current_dir(dir);
        let listed = crate::t3_update::output_within(list, CLI_TIME).and_then(|out| {
            if out.status.success() {
                cli_threads(&out.stdout, dir).map_err(|err| err.to_string())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
            }
        });
        match listed {
            Ok(found) => {
                for row in found {
                    if !rows.iter().any(|r| r.id == row.id) {
                        rows.push(row);
                    }
                }
            }
            // A folder of no project of T3 Code's, likely.
            Err(err) => tracing::debug!("t3 thread list, {}: {err}", dir.display()),
        }
    }
    rows
}

/// A thread as `t3 thread list --json` lists it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    thread_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    provider_instance_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    settled: Option<bool>,
    #[serde(default)]
    parent_thread_id: Option<String>,
    #[serde(default)]
    relationship_to_parent: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

/// The threads worth a card in what `t3 thread list --json` printed for
/// the project of folder `dir` (newest first): those at work or waiting,
/// then `CLI_IDLE` more; none settled.
fn cli_threads(json: &[u8], dir: &Path) -> serde_json::Result<Vec<ThreadRow>> {
    let listed: Vec<Listed> = serde_json::from_slice(json)?;
    let mut idle = 0;
    Ok(listed
        .into_iter()
        .filter(|t| t.settled != Some(true))
        .map(|t| ThreadRow {
            id: t.thread_id,
            title: t.title.unwrap_or_default(),
            updated: t.updated_at.unwrap_or_default(),
            project: Some(dir.to_owned()),
            worktree: None,
            branch: None,
            child: t.relationship_to_parent.as_deref() == Some("subagent")
                || t.parent_thread_id.is_some(),
            parent: t.parent_thread_id,
            provider: t.provider_instance_id,
            model: t.model,
            run: t.status,
            completed: None,
            visited: None,
            pending: 0,
            opened: None,
            first_run: t.created_at,
        })
        .filter(|row| {
            row.status() != AgentStatus::Idle || {
                idle += 1;
                idle <= CLI_IDLE
            }
        })
        .collect())
}

/// The database at `db`, opened to read only: T3 Code writes it meanwhile.
pub fn open_read_only(db: &Path) -> rusqlite::Result<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(db, flags)?;
    conn.busy_timeout(Duration::from_millis(100))?;
    Ok(conn)
}

/// The `t3` that runs `server`, out of T3 Code's home `home`: the version
/// it keeps there, else the one on PATH.
fn t3_binary(home: &Path, server: &Server) -> Option<PathBuf> {
    let kept = server
        .version
        .as_ref()
        .map(|v| home.join("runtime/versions").join(v).join("t3"))
        .filter(|path| path.is_file());
    kept.or_else(|| crate::desktop::installed("t3").then(|| PathBuf::from("t3")))
}

/// What to do for a web pane back at T3 Code's sign-in page, sent
/// `tries` pairing links since it last got past it.
#[derive(Debug, PartialEq, Eq)]
enum Pairing {
    Send,
    /// Say once that it doesn't take.
    GiveUp,
    Quiet,
}

fn pairing_turn(tries: u32) -> Pairing {
    match tries {
        t if t < PAIR_TRIES => Pairing::Send,
        t if t == PAIR_TRIES => Pairing::GiveUp,
        _ => Pairing::Quiet,
    }
}

/// Whether `path` (of a T3 Code page) is its sign-in page waiting for a
/// token: not a pairing link, which brings its own.
fn asks_to_pair(path: &str) -> bool {
    let page = path.split(['?', '#']).next().unwrap_or_default();
    page.trim_end_matches('/') == "/pair" && !path.contains("token=")
}

/// What to do for web pane `pane` of T3 Code's now at `path`, if it is
/// signed out; `paired` holds when each was last sent a pairing link or seen
/// taking one, and how many it was sent since it got past the sign-in page.
/// `busy`: a link is being asked for it already.
///
/// A page taking a link drops the token from its address, so it is at the
/// bare sign-in page for a moment, and then at the app. That isn't asking,
/// so the time is kept once it is past the page: a link sent for that moment
/// would bring the page back there, and so on for good.
fn pair_turn_at(
    paired: &mut HashMap<u32, (Instant, u32)>,
    pane: u32,
    path: &str,
    busy: bool,
    now: Instant,
) -> Option<Pairing> {
    let page = path.split(['?', '#']).next().unwrap_or_default();
    if !asks_to_pair(path) {
        if page.trim_end_matches('/') == "/pair" {
            paired.entry(pane).or_insert((now, 0)).0 = now;
        } else if let Some(sent) = paired.get_mut(&pane) {
            // Signed in: a later sign-out starts counting anew.
            sent.1 = 0;
        }
        return None;
    }
    // Signed out (a browser never paired, or the server forgot it): sign
    // it in, whichever way it got here.
    let sent = paired.get(&pane).copied();
    if busy || sent.is_some_and(|(at, _)| now.duration_since(at) < PAIR_AGAIN) {
        return Some(Pairing::Quiet);
    }
    let tries = sent.map_or(0, |(_, tries)| tries);
    let turn = pairing_turn(tries);
    if turn != Pairing::Quiet {
        paired.insert(pane, (now, tries + 1));
    }
    Some(turn)
}

/// Pairing link `link` with its token in the query, not the fragment: a
/// pane at the sign-in page already would only change its fragment, and
/// the page reads the token as it loads only. T3 Code's page takes either.
fn token_in_query(link: &str) -> String {
    let Some((page, fragment)) = link.split_once('#') else {
        return link.to_owned();
    };
    let (token, rest): (Vec<&str>, Vec<&str>) = fragment
        .split('&')
        .partition(|part| part.starts_with("token="));
    let Some(token) = token.first() else {
        return link.to_owned();
    };
    let joint = if page.contains('?') { '&' } else { '?' };
    let mut out = format!("{page}{joint}{token}");
    if !rest.is_empty() {
        out.push('#');
        out.push_str(&rest.join("&"));
    }
    out
}

/// A one-time link that signs a browser in to `server` (by `t3 pair`).
fn pairing_link(home: &Path, server: &Server) -> Result<String, String> {
    let binary = t3_binary(home, server).ok_or_else(|| "no t3 command found".to_owned())?;
    let out = std::process::Command::new(&binary)
        .arg("pair")
        .arg("--label")
        .arg("gerdr")
        .arg("--base-dir")
        .arg(home)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("{}: {err}", binary.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let link = text
        .lines()
        .find_map(|line| line.trim().strip_prefix(PAIRING_URL))
        .map(|link| crate::feature::verify::strip_escapes(link.trim()))
        .filter(|link| link.starts_with("http"));
    match link {
        Some(link) => Ok(link),
        None => {
            let err = String::from_utf8_lossy(&out.stderr);
            let why = err.lines().last().unwrap_or("no pairing link").trim();
            Err(format!("t3 pair: {why}"))
        }
    }
}

/// Whether T3 Code at `home` has a project at folder `dir`; why not, when
/// its database can't say.
fn has_project(home: &Path, dir: &Path) -> Result<bool, String> {
    let conn = open_read_only(&home.join(DB)).map_err(|err| err.to_string())?;
    let found = conn.query_row(
        "SELECT 1 FROM projection_projects WHERE workspace_root = ?1 AND deleted_at IS NULL",
        [dir.to_string_lossy()],
        |_| Ok(()),
    );
    match found {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(err) => Err(err.to_string()),
    }
}

/// How many projects T3 Code at `home` has, if its database says (why
/// not is logged).
fn project_count(home: &Path) -> Option<i64> {
    let sql = "SELECT COUNT(*) FROM projection_projects WHERE deleted_at IS NULL";
    let count =
        open_read_only(&home.join(DB)).and_then(|conn| conn.query_row(sql, [], |row| row.get(0)));
    count
        .map_err(|err| tracing::warn!("t3 code: counting its projects: {err}"))
        .ok()
}

/// What came of looking for a workspace's folder in T3 Code's projects
/// (see `Gerdr::add_t3_project`).
enum ProjectAdd {
    /// It was one already.
    Known,
    Added,
    Failed(String),
    /// T3 Code's database couldn't say whether it was one.
    Unreadable,
}

/// What went wrong for `reason` (of the extension's `scopeT3`) when T3
/// Code's app didn't take its project filter, in plain words: the reason
/// itself is for the log.
pub fn unscoped_words(reason: &str) -> &'static str {
    match reason {
        "no-react" => "gerdr can't find T3 Code's app in its page",
        "no-groups" => "gerdr can't find T3 Code's project list",
        "no-roots" => "T3 Code's projects don't say their folders",
        "ignored" => "T3 Code doesn't follow the filter gerdr sets",
        "overwritten" => "T3 Code undoes the filter gerdr sets",
        "error" => "gerdr's filter failed in T3 Code's page",
        _ => "T3 Code's page changed",
    }
}

/// Add folder `dir` as a T3 Code project of `server` (by `t3 project add`).
/// How long `t3 project add` may take.
const ADD_PROJECT_TIMEOUT: Duration = Duration::from_secs(30);

fn add_project(home: &Path, server: &Server, dir: &Path) -> Result<(), String> {
    let binary = t3_binary(home, server).ok_or_else(|| "no t3 command found".to_owned())?;
    let mut cmd = std::process::Command::new(&binary);
    cmd.args(["project", "add", "--base-dir"])
        .arg(home)
        .arg(dir);
    // It only talks to the server on this machine: one stuck is stopped.
    let out = crate::t3_update::output_within(cmd, ADD_PROJECT_TIMEOUT)?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(err.lines().last().unwrap_or("failed").trim().to_owned())
}

/// What gerdr follows of T3 Code.
#[derive(Default)]
pub struct T3 {
    /// Its home, if it ever ran here.
    pub home: Option<PathBuf>,
    pub server: Option<Server>,
    /// Whether the server runs.
    pub alive: bool,
    pub rows: Vec<ThreadRow>,
    /// The cards: threads doing something or with news, then idle ones,
    /// none hidden.
    pub threads: Vec<T3Thread>,
    /// The link to show once a pane's browser is signed in.
    pending: Option<String>,
    /// The new thread asked of a workspace, started once a pane of it
    /// shows the app (see `new_t3_thread`).
    new_thread: Option<NewThread>,
    /// Whether the watcher read the threads yet.
    read: bool,
    /// The web panes a pairing link is being asked for, by pane (`None`
    /// for a new tab).
    pairing: HashSet<Option<u32>>,
    /// The web panes the extension was told the folders of, for T3 Code's
    /// project filter (see `Gerdr::scope_t3`): the app's origin and the
    /// folders each was told, told again once they change.
    scoped: HashMap<u32, (String, Vec<PathBuf>)>,
    /// The folders T3 Code has as projects, or that gerdr added: looked
    /// at once each.
    checked: HashSet<PathBuf>,
    /// The folders being looked at, or added, now.
    adding: HashSet<PathBuf>,
    /// Whether the user heard that gerdr can't read T3 Code's projects.
    told_unreadable: bool,
    /// Why the app didn't take its project filter, each told once (see
    /// `Gerdr::t3_unscoped`).
    unscoped: HashSet<String>,
    /// The T3 Code version whose app last didn't take its project filter,
    /// and why: the footer shows it while that version runs.
    pub unscoped_in: Option<(Option<String>, String)>,
    /// When each web pane last landed on T3 Code's sign-in page and was
    /// sent a pairing link: once in a while only, should one not take.
    /// How many links it was sent since it last got past the sign-in page.
    paired_panes: HashMap<u32, (Instant, u32)>,
    /// Where the threads are read from.
    pub source: Source,
    /// The migration the database is at, if it says.
    pub migration: Option<i64>,
    /// The workspaces' folders, for asking the server of their threads.
    dirs: Arc<Mutex<Vec<PathBuf>>>,
    /// The threads held done (`Store::t3_finished`), for the watcher to
    /// read however old they are.
    held: Arc<Mutex<Vec<String>>>,
    /// What gerdr decided about T3 Code's versions (see `t3_update`).
    pub updates: crate::t3_update::Updates,
    /// Whether newer versions are taken as soon as they fit, else only
    /// when asked: Settings' `[t3] updates`.
    pub auto_update: bool,
    /// Whether a newer version is being looked for.
    pub checking: bool,
    /// The version the server restarts at once its threads are idle.
    pub restart_at: Option<String>,
    /// Whether the user heard that gerdr reads T3 Code in part.
    told_partial: bool,
    /// When gerdr last started the server, while it may yet fail to (see
    /// `t3_server`).
    pub started_at: Option<std::time::Instant>,
    /// Whether the server is being stopped.
    pub stopping: bool,
    /// The lines sent to each thread that `t3 thread send` hasn't taken
    /// yet, oldest first: one that fails is kept with the AI work.
    told: HashMap<String, VecDeque<crate::agent_tell::Told>>,
}

impl T3 {
    /// `told` goes to `thread`.
    fn telling(&mut self, thread: &str, told: crate::agent_tell::Told) {
        self.told
            .entry(thread.to_owned())
            .or_default()
            .push_back(told);
    }

    /// The oldest line going to `thread`, now that a send to it ended.
    fn told(&mut self, thread: &str) -> Option<crate::agent_tell::Told> {
        let queue = self.told.get_mut(thread)?;
        let told = queue.pop_front();
        if queue.is_empty() {
            self.told.remove(thread);
        }
        told
    }
}

impl crate::Gerdr {
    /// Follow T3 Code's threads, if it ran here: read its database off the
    /// event loop, and where its server runs every `SERVER_INTERVAL`.
    pub fn watch_t3_agents(&mut self) {
        use smithay::reexports::calloop::{
            channel,
            timer::{TimeoutAction, Timer},
        };
        let Some(home) = home() else {
            return;
        };
        self.t3.server = Server::read(&home);
        self.t3.alive = self.t3.server.as_ref().is_some_and(Server::alive);
        self.t3.home = Some(home.clone());
        self.t3_dirs();
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(snapshot) = event {
                state.t3_read(snapshot);
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("t3 code's agents: {err}");
        }
        self.t3_held();
        watch_db(
            home.clone(),
            self.t3.dirs.clone(),
            self.t3.held.clone(),
            move |s| tx.send(s).is_ok(),
        );
        self.watch_t3_updates();
        let tick = move |_, _: &mut (), state: &mut crate::Gerdr| {
            state.t3_dirs();
            let server = Server::read(&home);
            let alive = server.as_ref().is_some_and(Server::alive);
            if server != state.t3.server || alive != state.t3.alive {
                // The footer shows a broken filter while its version runs:
                // the problem goes with it.
                let version = server.as_ref().and_then(|s| s.version.as_ref());
                let broken = state.t3.unscoped_in.as_ref().map(|(v, _)| v.as_ref());
                if broken.is_some_and(|v| v != version) {
                    state.t3_forget_unscoped();
                }
                state.t3.server = server;
                state.t3.alive = alive;
                state.t3_changed();
            }
            TimeoutAction::ToDuration(SERVER_INTERVAL)
        };
        let timer = Timer::from_duration(SERVER_INTERVAL);
        if let Err(err) = self.loop_handle.insert_source(timer, tick) {
            tracing::warn!("t3 code's server: {err}");
        }
    }

    /// Note the threads held done for the watcher (see `read_rows`).
    fn t3_held(&self) {
        if let Ok(mut shared) = self.t3.held.lock() {
            let stages = self.features.all.values().flatten();
            let stages = stages
                .flat_map(|f| f.runs.values())
                .filter_map(|r| r.t3_thread());
            *shared = (self.projects.t3_finished.keys().map(String::as_str))
                .chain(stages)
                .map(str::to_owned)
                .collect();
        }
    }

    /// Note the workspaces' folders for the watcher (see `cli_rows`).
    fn t3_dirs(&self) {
        let dirs = self.workspaces.iter().map(|ws| ws.dir.clone()).collect();
        if let Ok(mut shared) = self.t3.dirs.lock() {
            *shared = dirs;
        }
    }

    /// Take in what the watcher read; say once when gerdr starts reading
    /// T3 Code in part.
    fn t3_read(&mut self, snapshot: Snapshot) {
        let partial = matches!(snapshot.source, Source::Cli(_));
        // Kept in the problems list while it lasts.
        crate::problems::set(T3_PROBLEMS, partial_problem(&snapshot.source));
        let was = std::mem::replace(&mut self.t3.source, snapshot.source);
        self.t3.rows = snapshot.rows;
        self.t3.read = true;
        self.t3.migration = snapshot.migration;
        if partial && was == Source::Database && !self.t3.told_partial {
            self.t3.told_partial = true;
            self.t3_reads_in_part();
        }
        self.t3_changed();
    }

    /// Say that gerdr shows T3 Code's threads with basic status only,
    /// offering the version pinned if that one is read in full.
    fn t3_reads_in_part(&mut self) {
        let running = self.t3.server.as_ref().and_then(|s| s.version.clone());
        // Short, as a toast is; the launcher's "Update T3 Code" says more.
        let text = "T3 Code changed, so gerdr shows only basic status for its threads".to_owned();
        let updates = crate::t3_update::Updates::load();
        let back = updates.pin.filter(|pin| {
            Some(pin) != running.as_ref()
                && matches!(
                    updates.verdicts.get(pin),
                    Some(crate::t3_update::Verdict::Fits { .. })
                )
        });
        let action = back.map(|pin| {
            let label = format!("Go back to {pin}");
            (label, crate::menu::MenuAction::UpdateT3(Some(pin)))
        });
        self.toast_with(text, crate::toast::Level::Warning, action);
    }

    /// Take in what T3 Code's threads do now, and notify of what is news.
    pub(crate) fn t3_changed(&mut self) {
        let all = threads(&self.t3.rows, self.t3.alive);
        let mut changed = self.projects.t3_hidden.settle(&all);
        let hidden = &self.projects.t3_hidden;
        let mut threads: Vec<T3Thread> = all.into_iter().filter(|t| !hidden.hides(t)).collect();
        let seen = self.t3_seen_thread();
        let mine: HashSet<String> = threads
            .iter()
            .filter(|t| self.t3_thread_place(t).is_some())
            .map(|t| t.row.id.clone())
            .collect();
        let in_panes = self.t3_pane_threads();
        let sight = Sight {
            alive: self.t3.alive,
            seen: seen.as_deref(),
            mine: &mine,
            in_panes: &in_panes,
        };
        changed |= hold_finished(
            &mut self.projects.t3_finished,
            &self.t3.threads,
            &mut threads,
            &sight,
        );
        if changed {
            self.projects.save();
        }
        self.t3_held();
        self.t3.threads = threads;
        self.notify_agents();
        self.t3_maybe_restart();
    }

    /// Hide T3 Code thread `id` from gerdr (see `Hidden::hide`).
    pub fn delete_t3_thread(&mut self, id: &str) {
        let Some(thread) = self.t3.threads.iter().find(|t| t.row.id == id) else {
            return;
        };
        self.projects.t3_hidden.hide(thread);
        self.projects.save();
        self.t3_changed();
    }

    /// The workspace of the shown project thread `thread` goes under (see
    /// `cursor_agents::place`): by its folders, else, for a thread of the
    /// project, the one showing T3 Code's app, or the first.
    pub fn t3_thread_place(&self, thread: &T3Thread) -> Option<usize> {
        use crate::cursor_agents::{Spot, place};
        let spots: Vec<Spot> = self
            .workspaces
            .iter()
            .map(|ws| Spot {
                dir: &ws.dir,
                worktrees: self.git.status.get(&ws.dir).map_or_else(Vec::new, |git| {
                    git.worktrees.iter().map(|w| w.path.as_path()).collect()
                }),
                fallback: false,
            })
            .collect();
        let mut dirs: Vec<&Path> = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .into_iter()
            .flat_map(|p| p.workspaces.iter().map(|r| r.dir.as_path()))
            .collect();
        dirs.extend(self.workspaces.iter().map(|ws| ws.dir.as_path()));
        let home = crate::home_dir();
        let paths = thread.row.paths();
        if let Some(w) = place(&paths, &spots, &dirs, home.as_deref()) {
            return Some(w);
        }
        // As `place` tells a chat of the project.
        let ours = paths.iter().any(|path| {
            !home.as_deref().is_some_and(|home| home.starts_with(path))
                && dirs.iter().any(|dir| {
                    Some(*dir) != home.as_deref()
                        && (path.starts_with(dir) || dir.starts_with(path))
                })
        });
        let app = self.t3_pane().map(|(w, ..)| w);
        (ours && !self.workspaces.is_empty()).then(|| app.unwrap_or(0))
    }

    /// The cards of T3 Code's threads under workspace `w`: those doing
    /// something, then the latest idle ones, `HISTORY_CARDS` at most.
    pub fn t3_cards(&self, w: usize) -> Vec<crate::chrome::AgentView> {
        let ours: Vec<&T3Thread> = self
            .t3
            .threads
            .iter()
            .filter(|t| self.t3_thread_place(t) == Some(w))
            .collect();
        let (idle, live): (Vec<&T3Thread>, Vec<&T3Thread>) = ours
            .into_iter()
            .partition(|t| t.status == AgentStatus::Idle);
        live.into_iter()
            .chain(idle.into_iter().take(HISTORY_CARDS))
            .map(crate::chrome::AgentView::t3)
            .collect()
    }

    /// The statuses of the T3 Code threads under workspace `w`.
    pub fn t3_statuses(&self, w: usize) -> impl Iterator<Item = AgentStatus> + '_ {
        self.t3
            .threads
            .iter()
            .filter(move |t| self.t3_thread_place(t) == Some(w))
            .map(|t| t.status)
    }

    /// T3 Code's threads as notifications see them: seen while gerdr's
    /// window has the keyboard and shows the thread in the app's pane.
    pub fn t3_watched(&self) -> Vec<crate::agents::Watched> {
        let shown = self.shown_t3_thread();
        self.t3
            .threads
            .iter()
            .filter_map(|thread| {
                let w = self.t3_thread_place(thread)?;
                let card = crate::chrome::AgentView::t3(thread);
                let mut body = vec![self.workspaces[w].name.as_str()];
                body.extend(card.title());
                Some(crate::agents::Watched {
                    key: card.key.clone(),
                    status: thread.status,
                    label: card.label(),
                    body: body.join(" · "),
                    seen: self.notices.host_focused && shown.as_deref() == Some(&thread.row.id),
                    finished: None,
                })
            })
            .collect()
    }

    /// The thread the user looks at now: shown in T3 Code's app, in the
    /// shown tab's focused pane, while gerdr's window has the keyboard.
    fn t3_seen_thread(&self) -> Option<String> {
        self.notices
            .host_focused
            .then(|| self.shown_t3_thread())
            .flatten()
    }

    /// Let a thread held done (see `hold_finished`) go once the user
    /// looks at it. Taken once per frame.
    pub fn t3_seen(&mut self) {
        if self.projects.t3_finished.is_empty() {
            return;
        }
        let Some(id) = self.t3_seen_thread() else {
            return;
        };
        if self.projects.t3_finished.contains_key(&id) {
            // `t3_changed` lets it go, and saves.
            self.t3_changed();
        }
    }

    /// The thread the shown tab's focused pane shows in T3 Code's app.
    pub fn shown_t3_thread(&self) -> Option<String> {
        let server = self.t3.server.as_ref()?;
        let tab = self.tab()?;
        let id = tab.focused_id()?;
        let pane = tab.panes.iter().find(|p| p.id == id)?;
        let url = pane.app_command.as_deref()?;
        is_app(url, server).then(|| thread_of_link(url)).flatten()
    }

    /// The pane showing T3 Code's app (its workspace, tab, leaf and the
    /// link it shows), the shown workspace's first.
    pub(crate) fn t3_pane(&self) -> Option<(usize, usize, u32, String)> {
        let server = self.t3.server.as_ref()?;
        let mut found = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                for pane in &tab.panes {
                    let url = pane.app_command.as_deref().unwrap_or_default();
                    if crate::is_web_link(url) && is_app(url, server) {
                        found.push((w, t, pane.id, url.to_owned()));
                    }
                }
            }
        }
        let shown = found.iter().position(|(w, ..)| *w == self.active_workspace);
        let i = shown.unwrap_or(0);
        (i < found.len()).then(|| found.swap_remove(i))
    }

    /// The threads gerdr's panes show in T3 Code's app, in every workspace
    /// and tab, focused or not.
    fn t3_pane_threads(&self) -> HashSet<String> {
        let Some(server) = self.t3.server.as_ref() else {
            return HashSet::new();
        };
        self.workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| &tab.panes)
            .filter_map(|pane| pane.app_command.as_deref())
            .filter(|url| crate::is_web_link(url) && is_app(url, server))
            .filter_map(thread_of_link)
            .collect()
    }

    /// The pane of workspace `w` showing T3 Code's app (its tab, leaf and
    /// the link it shows).
    fn t3_pane_in(&self, w: usize) -> Option<(usize, u32, String)> {
        let server = self.t3.server.as_ref()?;
        let ws = self.workspaces.get(w)?;
        ws.tabs.iter().enumerate().find_map(|(t, tab)| {
            tab.panes.iter().find_map(|pane| {
                let url = pane.app_command.as_deref().unwrap_or_default();
                (crate::is_web_link(url) && is_app(url, server))
                    .then(|| (t, pane.id, url.to_owned()))
            })
        })
    }

    /// The origin workspace `w` shows T3 Code's app of `server` at (see
    /// `app_host`).
    fn t3_origin(&self, w: usize, server: &Server) -> String {
        match (self.workspaces.get(w), site(&server.origin)) {
            (Some(ws), Some((_, port))) => format!("http://{}:{port}", app_host(&ws.dir)),
            _ => server.origin.clone(),
        }
    }

    /// The folders whose T3 Code projects workspace `w`'s app shows: its
    /// own, with every project in it, so a workspace at a parent folder
    /// shows all the repositories under it.
    fn t3_scope(&self, w: usize) -> Vec<PathBuf> {
        self.workspaces
            .get(w)
            .map(|ws| vec![ws.dir.clone()])
            .unwrap_or_default()
    }

    /// The link web pane `pane` shows, if gerdr knows of it.
    fn web_pane_link(&self, pane: u32) -> Option<&str> {
        self.workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .find_map(|tab| {
                let shown = tab.panes.iter().find(|p| p.id == pane);
                match shown {
                    Some(p) => Some(p.app_command.as_deref()),
                    None => tab
                        .expected
                        .iter()
                        .find(|e| e.pane == pane)
                        .map(|e| e.app_command.as_deref()),
                }
            })
            .flatten()
    }

    /// Whether web pane `pane` shows T3 Code's app now.
    fn shows_t3_app(&self, pane: u32) -> bool {
        let (Some(server), Some(link)) = (&self.t3.server, self.web_pane_link(pane)) else {
            return false;
        };
        crate::is_web_link(link) && is_app(link, server)
    }

    /// Keep web pane `pane`, showing T3 Code's app at `origin`, to its
    /// workspace's projects: told to the extension, which applies it at
    /// each load at that origin, again once the pane is at another or in
    /// another workspace.
    fn scope_t3(&mut self, pane: u32, origin: &str) {
        let mut panes = HashSet::new();
        let mut place = None;
        for (w, ws) in self.workspaces.iter().enumerate() {
            for tab in &ws.tabs {
                let ids = tab.panes.iter().map(|p| p.id);
                for id in ids.chain(tab.expected.iter().map(|e| e.pane)) {
                    panes.insert(id);
                    if id == pane {
                        place = Some(w);
                    }
                }
            }
        }
        // Panes gone are forgotten.
        self.t3.scoped.retain(|id, _| panes.contains(id));
        let Some(w) = place else {
            return;
        };
        let told = (origin.to_owned(), self.t3_scope(w));
        if self.t3.scoped.get(&pane) == Some(&told) {
            return;
        }
        if self.scope_web_pane(pane, &told.0, &told.1) {
            self.t3.scoped.insert(pane, told);
        }
    }

    /// The workspace web pane `pane` is in.
    fn web_pane_workspace(&self, pane: u32) -> Option<usize> {
        self.workspaces.iter().position(|ws| {
            ws.tabs.iter().any(|tab| {
                tab.panes.iter().any(|p| p.id == pane)
                    || tab.expected.iter().any(|e| e.pane == pane)
            })
        })
    }

    /// Web pane `pane` shows T3 Code's app at `origin`: start the new
    /// thread asked of its workspace before it had the app open, if any.
    fn t3_start_asked(&mut self, pane: u32, origin: &str) {
        let Some(asked) = self.t3.new_thread.clone() else {
            return;
        };
        if asked.at.elapsed() > NEW_THREAD_WAIT {
            self.t3.new_thread = None;
            return;
        }
        let Some(w) = self.web_pane_workspace(pane) else {
            return;
        };
        if self.workspaces[w].dir != asked.dir {
            return;
        }
        self.t3.new_thread = None;
        let dirs = match asked.prompt {
            Some(_) => vec![asked.dir],
            None => self.t3_scope(w),
        };
        if !self.new_thread_in_web_pane(pane, origin, &dirs, asked.prompt.as_deref()) {
            self.t3_unsteered(w);
        }
    }

    /// A new thread for workspace `w` was left to T3 Code's own choice of
    /// project: gerdr's extension doesn't follow the pane (see
    /// `web_control`).
    fn t3_unsteered(&mut self, w: usize) {
        let name = self.workspaces.get(w).map_or("", |ws| ws.name.as_str());
        tracing::warn!("t3 code: web pane of {name} can't be steered: new thread left to T3 Code");
        let text = format!(
            "T3 Code's new thread may be in another project than {name}'s: gerdr can't steer \
             its web pane"
        );
        self.toast(text, crate::toast::Level::Warning);
    }

    /// T3 Code's app in web pane `pane` started no new thread in its
    /// workspace's project (see `inT3` in the extension's worker.js):
    /// `reason` says why in a word, `detail` what the page showed.
    pub fn t3_no_thread(&mut self, pane: u32, reason: &str, detail: &str) {
        let version = self.t3.server.as_ref().and_then(|s| s.version.as_deref());
        let version = version.unwrap_or("unknown");
        let name = self.web_pane_workspace(pane).map_or_else(
            || "this workspace".to_owned(),
            |w| self.workspaces[w].name.clone(),
        );
        let where_ = format!("web pane {pane} ({name}), T3 Code {version}");
        let open_log = ("Open log".to_owned(), crate::menu::MenuAction::OpenLog);
        match reason {
            // T3 Code works: the folder isn't one of its projects (yet).
            "no-match" => {
                tracing::warn!("t3 code: no new thread in {where_}: {detail}");
                let text = format!(
                    "T3 Code started no thread in {name}: none of its projects is in its folder"
                );
                self.toast_with(text, crate::toast::Level::Warning, Some(open_log));
            }
            // The app went elsewhere each time it was asked.
            "refused" => {
                tracing::warn!("t3 code: no new thread in {where_}: {detail}");
                let text = format!("T3 Code didn't start a thread in {name}: try again");
                self.toast(text, crate::toast::Level::Warning);
            }
            _ => {
                tracing::error!(
                    "t3 code: no new thread in {where_}: {reason}: {detail}. gerdr starts it in \
                     assets/web-extension/worker.js `inT3`, which reads the app's React tree, \
                     not T3 Code's API: this T3 Code version likely changed it"
                );
                let text = format!(
                    "Couldn't start a T3 Code thread in {name}: gerdr can't find how in \
                     T3 Code {version}"
                );
                self.toast_with(text, crate::toast::Level::Error, Some(open_log));
            }
        }
    }

    /// Web pane `pane` moved to another workspace: one showing T3 Code's
    /// app shows that one's projects now.
    pub(crate) fn t3_pane_moved(&mut self, pane: u32) {
        if !self.shows_t3_app(pane) {
            return;
        }
        let link = self.web_pane_link(pane).unwrap_or_default();
        let origin = origin_of(link).unwrap_or_default().to_owned();
        self.scope_t3(pane, &origin);
    }

    /// T3 Code's app in web pane `pane` took its project filter: the
    /// filter works in this version again.
    pub fn t3_scoped(&mut self, pane: u32) {
        if !self.shows_t3_app(pane) || self.t3.unscoped_in.is_none() {
            return;
        }
        tracing::info!("t3 code: project filter set in web pane {pane}");
        self.t3_forget_unscoped();
    }

    /// Let go of a project filter the app didn't take: it does now, or
    /// another version runs.
    fn t3_forget_unscoped(&mut self) {
        self.t3.unscoped_in = None;
        // A workspace without projects is told of once still.
        self.t3.unscoped.retain(|reason| reason == "no-match");
        crate::problems::clear(FILTER_PROBLEMS);
    }

    /// Whether T3 Code has no projects at all, as its database says.
    fn t3_has_no_projects(&self) -> bool {
        let home = self.t3.home.as_deref();
        home.and_then(project_count) == Some(0)
    }

    /// T3 Code's app in web pane `pane` didn't take its project filter,
    /// for `reason` (see the extension's `scopeT3`): logged in full, for
    /// whoever fixes gerdr for the T3 Code version that changed it, and
    /// told once per reason. A pane that left the app since, or the
    /// extension's late word of a page it left, is let be.
    pub fn t3_unscoped(&mut self, pane: u32, reason: &str, detail: &str) {
        if !self.shows_t3_app(pane) {
            tracing::info!("t3 code: web pane {pane} left the app: {reason} let be: {detail}");
            return;
        }
        // An app without projects shows no list of them.
        let reason = if reason == "no-groups" && self.t3_has_no_projects() {
            "no-match"
        } else {
            reason
        };
        let version = self.t3.server.as_ref().and_then(|s| s.version.as_deref());
        let version = version.unwrap_or("unknown");
        let place = self
            .workspaces
            .iter()
            .find(|ws| ws.tabs.iter().any(|t| t.panes.iter().any(|p| p.id == pane)));
        let name = place.map_or_else(|| "this workspace".to_owned(), |ws| ws.name.clone());
        let where_ = format!("web pane {pane} ({name}), T3 Code {version}");
        // No project in the folder: T3 Code works, the folder isn't one
        // of its projects.
        if reason == "no-match" {
            tracing::warn!("t3 code: project filter not set in {where_}: {detail}");
            if self.t3.unscoped.insert(reason.to_owned()) {
                let text =
                    format!("T3 Code shows all projects in {name}: none of them is in its folder");
                self.toast(text, crate::toast::Level::Warning);
            }
            return;
        }
        tracing::error!(
            "t3 code: project filter not set in {where_}: {reason}: {detail}. \
             gerdr sets it in assets/web-extension/worker.js `scopeT3`, which reads the app's \
             React tree and its localStorage, neither of them T3 Code's API: this T3 Code \
             version likely changed them"
        );
        let running = self.t3.server.as_ref().and_then(|s| s.version.clone());
        self.t3.unscoped_in = Some((running, reason.to_owned()));
        crate::problems::report(
            FILTER_PROBLEMS,
            format!(
                "T3 Code {version} doesn't take gerdr's project filter: {} (see the log)",
                unscoped_words(reason)
            ),
        );
        if !self.t3.unscoped.insert(reason.to_owned()) {
            return;
        }
        let text = format!(
            "T3 Code shows all projects in {name}: gerdr can't set its project filter \
             in T3 Code {version}"
        );
        let open_log = ("Open log".to_owned(), crate::menu::MenuAction::OpenLog);
        self.toast_with(text, crate::toast::Level::Error, Some(open_log));
    }

    /// Add workspace `w`'s folder as a T3 Code project when it is a
    /// repository T3 Code doesn't know yet (off the event loop; once per
    /// folder): its app can only show, and start, threads of projects.
    fn add_t3_project(&mut self, w: usize, server: &Server) {
        let Some(ws) = self.workspaces.get(w) else {
            return;
        };
        let dir = ws.dir.clone();
        if !dir.join(".git").exists() || Some(dir.as_path()) == crate::home_dir().as_deref() {
            return;
        }
        self.ensure_t3_project(dir, server);
    }

    /// Make folder `dir` a project of T3 Code's, off the event loop.
    fn ensure_t3_project(&mut self, dir: PathBuf, server: &Server) {
        let Some(home) = self.t3.home.clone() else {
            return;
        };
        // Looked at again until T3 Code has it.
        if self.t3.checked.contains(&dir) || !self.t3.adding.insert(dir.clone()) {
            return;
        }
        use crate::toast::Level;
        use smithay::reexports::calloop::channel;
        let (tx, rx) = channel::channel::<(PathBuf, ProjectAdd)>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            let channel::Event::Msg((dir, done)) = event else {
                return;
            };
            state.t3.adding.remove(&dir);
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            let name = name.into_owned();
            let open_log = || Some(("Open log".to_owned(), crate::menu::MenuAction::OpenLog));
            match done {
                ProjectAdd::Known => {
                    state.t3.checked.insert(dir);
                }
                ProjectAdd::Added => {
                    state.t3.checked.insert(dir);
                    state.toast(
                        format!("Added {name} to T3 Code's projects"),
                        Level::Success,
                    );
                }
                ProjectAdd::Failed(err) => {
                    let text = format!("Couldn't add {name} to T3 Code's projects: {err}");
                    state.toast_with(text, Level::Error, open_log());
                }
                ProjectAdd::Unreadable => {
                    if !std::mem::replace(&mut state.t3.told_unreadable, true) {
                        let text = format!(
                            "Couldn't read T3 Code's projects, so gerdr didn't add {name} to them"
                        );
                        state.toast_with(text, Level::Warning, open_log());
                    }
                }
            }
        });
        if let Err(err) = inserted {
            self.t3.adding.remove(&dir);
            return tracing::warn!("t3 code: {err}");
        }
        let server = server.clone();
        std::thread::spawn(move || {
            let done = match has_project(&home, &dir) {
                Ok(true) => ProjectAdd::Known,
                Ok(false) => match add_project(&home, &server, &dir) {
                    Ok(()) => {
                        tracing::info!("t3 code: added project {}", dir.display());
                        ProjectAdd::Added
                    }
                    Err(err) => {
                        tracing::warn!("t3 code: adding {}: {err}", dir.display());
                        ProjectAdd::Failed(err)
                    }
                },
                Err(err) => {
                    tracing::warn!("t3 code: reading its projects for {}: {err}", dir.display());
                    ProjectAdd::Unreadable
                }
            };
            let _ = tx.send((dir, done));
        });
    }

    /// What T3 Code thread `id` does, if known, and whether it is still
    /// there. Its row gone means so only when the database was read; the
    /// server's list leaves out what isn't a workspace's. With the server
    /// down nothing is known of what it does.
    pub(crate) fn t3_thread_state(&self, id: &str) -> (Option<AgentStatus>, bool) {
        let all = threads(&self.t3.rows, self.t3.alive);
        match all.into_iter().find(|t| t.row.id == id) {
            Some(thread) => (self.t3.alive.then_some(thread.status), true),
            None => (None, !self.t3.read || self.t3.source != Source::Database),
        }
    }

    /// Whether a draft can be opened in T3 Code's app; offering to start
    /// its server, and why not, when one can't.
    pub(crate) fn t3_can_draft(&mut self) -> Result<(), String> {
        if self.t3.server.is_some() && self.t3.alive {
            return Ok(());
        }
        let action = ("Start".to_owned(), crate::menu::MenuAction::StartT3);
        let text = "T3 Code's server isn't running".to_owned();
        self.toast_with(text.clone(), crate::toast::Level::Warning, Some(action));
        Err(text)
    }

    /// `stage`'s agent of feature `slug` (`title`) got a run in T3 Code,
    /// waiting on its draft: shown when the user is at the feature, else
    /// told. A stage can start by itself, and a web page taking the
    /// keyboard from whatever the user does elsewhere would swallow it.
    pub(crate) fn t3_stage_drafted(
        &mut self,
        project: &str,
        slug: &str,
        stage: crate::feature::Stage,
        title: &str,
    ) {
        let at_feature =
            (self.features.panel.as_ref()).is_some_and(|p| p.project == project && p.slug == slug);
        if at_feature {
            self.show_t3_stage(project, slug, stage);
            return self.show_success(format!(
                "{} of \u{201c}{title}\u{201d} waits in T3 Code: pick a model and send it",
                stage.label()
            ));
        }
        let text = format!(
            "{} of \u{201c}{title}\u{201d} waits for you: Show agent on its page opens T3 Code",
            stage.label()
        );
        self.notify_feature(project, slug, None, &text);
        self.show_warning(text);
    }

    /// Show `stage`'s agent of feature `slug` in T3 Code's app, in the
    /// feature's workspace: its thread, or its draft until it is sent.
    pub(crate) fn show_t3_stage(
        &mut self,
        project: &str,
        slug: &str,
        stage: crate::feature::Stage,
    ) {
        use crate::feature::agents::T3Run;
        let Some(run) = self.feature(project, slug).and_then(|f| f.runs.get(&stage)) else {
            return;
        };
        match run.t3.clone() {
            Some(T3Run::Thread { id, .. }) => {
                self.in_feature_workspace(project, slug, |gerdr, w| {
                    gerdr.open_t3(w, Some(&id));
                });
            }
            Some(T3Run::Draft) => {
                let path = run.prompt.clone();
                let prompt = match std::fs::read_to_string(&path) {
                    Ok(prompt) => prompt,
                    Err(err) => {
                        tracing::warn!("reading {}: {err}", path.display());
                        return self.show_error(format!(
                            "Couldn't read {}'s prompt: {err}",
                            stage.label()
                        ));
                    }
                };
                self.in_feature_workspace(project, slug, |gerdr, w| {
                    gerdr.show_t3(w, Page::Draft(&prompt));
                });
            }
            None => {}
        }
    }

    /// When `run` of `stage` of `feature` waits on its draft and the thread
    /// sent from it is there now, the run follows that thread. Whether it
    /// did.
    pub(crate) fn t3_stage_thread_sent(
        &mut self,
        feature: &crate::feature::Feature,
        stage: crate::feature::Stage,
        run: &crate::feature::agents::StageRun,
    ) -> bool {
        use crate::feature::agents::T3Run;
        if run.t3 != Some(T3Run::Draft) {
            return false;
        }
        let taken: Vec<&str> = (feature.runs.values())
            .filter_map(|r| r.t3_thread())
            .collect();
        let found = drafted_thread(&self.t3.rows, &feature.dir, run.started, &taken);
        let Some(row) = found.cloned() else {
            return false;
        };
        let (project, slug) = (feature.project.clone(), feature.slug.clone());
        let agent = row.agent();
        let same = (stage == crate::feature::Stage::AiReview)
            .then(|| self.projects.project(&project).map(|p| &p.workflow))
            .and_then(|workflow| {
                crate::feature_agents::t3_reviews_its_build(
                    feature,
                    workflow,
                    &self.launcher.cli_agents,
                    &agent,
                )
            });
        let Some(f) = self.feature_mut(&project, &slug) else {
            return false;
        };
        let Some(r) = f.runs.get_mut(&stage) else {
            return false;
        };
        r.t3 = Some(T3Run::Thread {
            id: row.id,
            agent: Some(agent),
        });
        r.model = row.model.clone();
        let model = row.model.unwrap_or_else(|| "its model".to_owned());
        f.log(
            crate::feature::now(),
            format!("{} agent runs in T3 Code: {model}", stage.label()),
        );
        self.save_feature(&project, &slug);
        self.t3_held();
        let key = (project, slug, stage);
        if let Some(watch) = self.features.agents.get_mut(&key) {
            watch.worked = true;
        }
        if let Some(builder) = same {
            self.show_warning(format!(
                "T3 Code reviews with {builder}, which built this code: a model misses what it missed"
            ));
        }
        true
    }

    /// Send T3 Code thread `id` the line `told`, as its next turn when it
    /// is busy, off the event loop. Why not, when it can't be tried.
    pub(crate) fn t3_send(
        &mut self,
        id: &str,
        told: &crate::agent_tell::Told,
    ) -> Result<(), String> {
        let home = self.t3.home.clone();
        let server = self.t3.server.clone().filter(|_| self.t3.alive);
        let (Some(home), Some(server)) = (home, server) else {
            return Err("T3 Code's server isn't running".into());
        };
        let binary = t3_binary(&home, &server).ok_or_else(|| "no t3 command found".to_owned())?;
        let (thread, text) = (id.to_owned(), told.text.clone());
        if self.ai_work.is_on() {
            self.t3.telling(id, told.clone());
        }
        self.feature_work(None, move |_| {
            let mut send = std::process::Command::new(&binary);
            send.args(["thread", "send", "--base-dir"])
                .arg(&home)
                .arg(&thread)
                .arg(&text);
            let result = crate::t3_update::output_within(send, CLI_TIME).and_then(|out| {
                if out.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
                }
            });
            crate::features::Done::T3Told { thread, result }
        });
        Ok(())
    }

    /// `t3 thread send` to `thread` ended.
    pub(crate) fn t3_told(&mut self, thread: &str, result: Result<(), String>) {
        let told = self.t3.told(thread);
        if let Err(why) = result {
            tracing::warn!("t3 code: sending thread {thread} a message: {why}");
            if let Some(told) = told {
                self.told_failed(&told, crate::ai_work::Via::T3, &why);
            }
            self.show_error(format!("Couldn't message T3 Code's thread: {why}"));
        }
    }

    /// A feature's folder at `dir` goes: so does its project in T3 Code,
    /// if T3 Code has no thread there. Those threads are the user's to
    /// keep, so `t3 project remove` refusing is only logged, as is what
    /// can't be tried with the server down: the project then stays, a
    /// folder T3 Code can't open, for the user to remove there.
    pub(crate) fn t3_forget_project(&mut self, dir: &Path) {
        let home = self.t3.home.clone();
        let server = self.t3.server.clone().filter(|_| self.t3.alive);
        self.t3.checked.remove(dir);
        let (Some(home), Some(server)) = (home, server) else {
            return tracing::info!("t3 code: left project {} as it is", dir.display());
        };
        let dir = dir.to_owned();
        std::thread::spawn(move || {
            if !matches!(has_project(&home, &dir), Ok(true)) {
                return;
            }
            let Some(binary) = t3_binary(&home, &server) else {
                return tracing::warn!("t3 code: no t3 command to remove {}", dir.display());
            };
            let mut remove = std::process::Command::new(binary);
            remove
                .args(["project", "remove", "--base-dir"])
                .arg(&home)
                .arg(&dir);
            match crate::t3_update::output_within(remove, CLI_TIME) {
                Ok(out) if out.status.success() => {
                    tracing::info!("t3 code: removed project {}", dir.display());
                }
                Ok(out) => tracing::info!(
                    "t3 code: kept project {}: {}",
                    dir.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                Err(err) => tracing::warn!("t3 code: removing {}: {err}", dir.display()),
            }
        });
    }

    /// Show T3 Code thread `id` in the app's pane.
    pub fn focus_t3_thread(&mut self, id: &str) {
        let w = self
            .t3
            .threads
            .iter()
            .find(|t| t.row.id == id)
            .and_then(|t| self.t3_thread_place(t))
            .unwrap_or(self.active_workspace);
        self.open_t3(w, Some(id));
    }

    /// Show T3 Code's app, at thread `thread` if any: in the pane showing
    /// it already, else in a new tab of workspace `w`, signing the
    /// project's browser in first if it never was.
    pub fn open_t3(&mut self, w: usize, thread: Option<&str>) {
        self.show_t3(w, thread.map_or(Page::App, Page::Thread));
    }

    /// Start a new thread in T3 Code's app for workspace `w`, as a new
    /// agent: a draft of the workspace's project, in its pane showing the
    /// app or a new tab, by gerdr's extension (see `inT3` in its
    /// worker.js); without it, the app's home page, which opens a draft in
    /// the project it likes.
    pub fn new_t3_thread(&mut self, w: usize) {
        self.show_t3(w, Page::New);
    }

    /// Show T3 Code's app at `page` for workspace `w` (see `open_t3`).
    fn show_t3(&mut self, w: usize, page: Page) {
        use crate::toast::Level;
        let Some(server) = self.t3.server.clone().filter(|_| self.t3.alive) else {
            let action = ("Start".to_owned(), crate::menu::MenuAction::StartT3);
            let text = "T3 Code's server isn't running".to_owned();
            return self.toast_with(text, Level::Warning, Some(action));
        };
        let link = |origin: &str| match page {
            Page::Thread(id) => server.thread_link(origin, id),
            Page::App | Page::New | Page::Draft(_) => format!("{origin}/"),
        };
        let w = w.min(self.workspaces.len().saturating_sub(1));
        match (page, self.workspaces.get(w)) {
            (Page::Draft(_), Some(ws)) => self.ensure_t3_project(ws.dir.clone(), &server),
            _ => self.add_t3_project(w, &server),
        }
        let own = self.t3_origin(w, &server);
        if let Some((t, pane, url)) = self.t3_pane_in(w) {
            self.show_leaf(w, t, pane);
            // One at another workspace's host (moved here, or opened
            // before workspaces had their own) stays there: another host
            // is a new page, loaded and signed in again. It is kept to
            // this workspace's projects where it is.
            let here = origin_of(&url).unwrap_or(own.as_str()).to_owned();
            self.scope_t3(pane, &here);
            if page == Page::New {
                let dirs = self.t3_scope(w);
                if !self.new_thread_in_web_pane(pane, &here, &dirs, None) {
                    // Without gerdr's extension in its browser it can't be
                    // told to: a tab of its own at the app's home page,
                    // which opens a draft where the app likes.
                    self.launch(&link(&own), crate::launch::Place::Tab);
                    self.t3_unsteered(w);
                }
                return;
            }
            if let Page::Draft(prompt) = page {
                let dirs = vec![self.workspaces[w].dir.clone()];
                if !self.new_thread_in_web_pane(pane, &here, &dirs, Some(prompt)) {
                    self.t3_unsteered(w);
                }
                return;
            }
            let target = link(&here);
            if page != Page::App && url != target && !self.go_in_web_app(pane, &target) {
                // Without gerdr's extension in its browser it can't be
                // told where to go: a tab of its own then.
                self.launch(&target, crate::launch::Place::Tab);
            }
            return;
        }
        let prompt = match page {
            Page::Draft(prompt) => Some(prompt),
            _ => None,
        };
        if (page == Page::New || prompt.is_some())
            && let Some(ws) = self.workspaces.get(w)
        {
            self.t3.new_thread = Some(NewThread {
                dir: ws.dir.clone(),
                prompt: prompt.map(str::to_owned),
                at: Instant::now(),
            });
        }
        if w != self.active_workspace && w < self.workspaces.len() {
            self.switch_workspace(w);
        }
        let origin = own;
        let marker = self.t3_paired_marker(&origin);
        if marker.as_ref().is_some_and(|m| m.exists()) {
            return self.launch(&link(&origin), crate::launch::Place::Tab);
        }
        let then = link(&origin);
        self.pair_t3(server, origin, Some(then), marker, None);
    }

    /// Where gerdr notes that the project's browser was signed in to T3
    /// Code's app at `origin`.
    fn t3_paired_marker(&self, origin: &str) -> Option<PathBuf> {
        let (host, port) = site(origin)?;
        let host: String = host
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
            .collect();
        Some(self.instance_dir.join(format!("t3-paired-{host}-{port}")))
    }

    /// Sign the project's browser in to `server`'s app at `origin` by a
    /// one-time link (off the event loop: `t3 pair` takes a moment), then
    /// show `then`. The link opens in web pane `pane` when given, else in
    /// a new tab.
    fn pair_t3(
        &mut self,
        server: Server,
        origin: String,
        then: Option<String>,
        marker: Option<PathBuf>,
        pane: Option<u32>,
    ) {
        use smithay::reexports::calloop::channel;
        let Some(home) = self.t3.home.clone() else {
            return;
        };
        if self.t3.pairing.contains(&pane) {
            return;
        }
        let (tx, rx) = channel::channel::<Result<String, String>>();
        let inserted = self.loop_handle.insert_source(rx, move |event, _, state| {
            let channel::Event::Msg(result) = event else {
                return;
            };
            state.t3.pairing.remove(&pane);
            match result {
                Ok(pairing) => {
                    // At the app's host: `t3 pair` names its own.
                    let pairing = token_in_query(&at_origin(&pairing, &origin));
                    if let Some(marker) = &marker
                        && let Err(err) = crate::projects::write_atomic(marker, origin.as_bytes())
                    {
                        tracing::warn!("{}: {err}", marker.display());
                    }
                    state.t3.pending = then.clone();
                    match pane {
                        // A pane that can't be sent it isn't sent a tab
                        // instead, each time it is back at the page.
                        Some(pane) => {
                            if !state.navigate_web_pane(pane, &pairing) {
                                let text = "Couldn't sign the web pane in to T3 Code: \
                                            gerdr can't steer it";
                                state.toast(text.to_owned(), crate::toast::Level::Error);
                            }
                        }
                        None => state.launch(&pairing, crate::launch::Place::Tab),
                    }
                }
                Err(err) => {
                    tracing::warn!("t3 code: {err}");
                    let text = format!("Couldn't sign in to T3 Code: {err}");
                    state.toast(text, crate::toast::Level::Error);
                    if let Some(then) = &then {
                        state.launch(then, crate::launch::Place::Tab);
                    }
                }
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("t3 code: {err}");
        }
        self.t3.pairing.insert(pane);
        std::thread::spawn(move || {
            let _ = tx.send(pairing_link(&home, &server));
        });
    }

    /// Take in that web pane `pane` is at `url` now: once a browser just
    /// signed in to T3 Code leaves the sign-in page, show it what it was
    /// opened for.
    pub fn t3_page(&mut self, pane: u32, url: &str) {
        let Some(server) = &self.t3.server else {
            return;
        };
        if !is_app(url, server) {
            return;
        }
        let origin = origin_of(url).unwrap_or_default();
        let path = &url[origin.len()..];
        let busy = self.t3.pairing.contains(&Some(pane));
        let paired = &mut self.t3.paired_panes;
        if let Some(turn) = pair_turn_at(paired, pane, path, busy, Instant::now()) {
            match turn {
                Pairing::Send => {
                    let server = server.clone();
                    let origin = origin.to_owned();
                    let marker = self.t3_paired_marker(&origin);
                    self.pair_t3(server, origin, None, marker, Some(pane));
                }
                Pairing::GiveUp => {
                    let text = "T3 Code keeps asking this web pane to sign in: \
                                sign in on its page, or see T3 Code's log";
                    tracing::warn!("{text}");
                    let open_log = ("Open log".to_owned(), crate::menu::MenuAction::OpenLog);
                    self.toast_with(text.to_owned(), crate::toast::Level::Error, Some(open_log));
                }
                Pairing::Quiet => {}
            }
            return;
        }
        if !path.starts_with("/pair") {
            self.scope_t3(pane, origin);
            self.t3_start_asked(pane, origin);
        }
        if self.t3.pending.is_none() || path.starts_with("/pair") {
            return;
        }
        if let Some(then) = self.t3.pending.take() {
            // At the host it signed in at.
            let then_origin = origin_of(&then).unwrap_or_default().to_owned();
            let target = format!("{origin}{}", &then[then_origin.len()..]);
            if target != url {
                self.go_in_web_app(pane, &target);
            }
        }
    }

    /// The command starting T3 Code's server, at `port` if any (see
    /// `Start::command`), without opening a browser: gerdr shows its app.
    pub fn t3_command(&self, port: Option<u16>) -> String {
        let pin = crate::t3_update::Updates::load().pin;
        let start = Start {
            home: self.t3.home.as_deref(),
            pin: pin.as_deref(),
            port,
        };
        format!("{} --no-browser", start.command(crate::desktop::installed))
    }
}

/// How T3 Code's server is started.
struct Start<'a> {
    /// T3 Code's home.
    home: Option<&'a Path>,
    /// The version to run (see `t3_update`).
    pin: Option<&'a str>,
    /// The port to listen at; T3 Code's own choice without.
    port: Option<u16>,
}

impl Start<'_> {
    /// The command: at version `pin`, from T3 Code's home when downloaded
    /// there, else by npx; without a pin, `t3 serve` if `installed` says
    /// there is a `t3`, else the newest by npx. At the home and port
    /// given.
    fn command(&self, installed: impl Fn(&str) -> bool) -> String {
        let quote = |text: &str| crate::desktop::shell_quote(text);
        let mut command = match self.pin {
            Some(pin) => match self
                .home
                .filter(|home| crate::t3_update::installed(home, pin))
            {
                Some(home) => {
                    let bin = crate::t3_update::runtime(home, pin);
                    format!("{} serve", quote(&bin.to_string_lossy()))
                }
                None => format!("npx --yes t3@{pin} serve"),
            },
            None if installed("t3") => "t3 serve".to_owned(),
            None => "npx t3@latest serve".to_owned(),
        };
        if let Some(home) = self.home {
            command.push_str(&format!(" --base-dir {}", quote(&home.to_string_lossy())));
        }
        if let Some(port) = self.port {
            command.push_str(&format!(" --port {port}"));
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_that_ends_finds_its_line_by_thread() {
        let told = |n, text: &str| crate::agent_tell::Told {
            project: "p".into(),
            slug: "s".into(),
            stage: crate::feature::Stage::Build,
            n,
            kind: crate::ai_work::SendKind::FixOn,
            text: text.into(),
        };
        let mut t3 = T3::default();
        t3.telling("a", told(1, "one"));
        t3.telling("b", told(2, "other"));
        t3.telling("a", told(1, "two"));
        assert_eq!(t3.told("a"), Some(told(1, "one")));
        assert_eq!(t3.told("a"), Some(told(1, "two")));
        assert_eq!(t3.told("a"), None);
        assert_eq!(t3.told("b"), Some(told(2, "other")));
        assert!(t3.told.is_empty());
    }

    #[test]
    fn pairing_tokens_move_to_the_query() {
        let link = "http://app.localhost:3773/pair#token=AB12";
        assert_eq!(
            token_in_query(link),
            "http://app.localhost:3773/pair?token=AB12"
        );
        let link = "http://a.localhost:1/pair?x=1#token=AB&y=2";
        assert_eq!(
            token_in_query(link),
            "http://a.localhost:1/pair?x=1&token=AB#y=2"
        );
        let plain = "http://a.localhost:1/pair#y=2";
        assert_eq!(token_in_query(plain), plain);
        // Still a pairing link to `t3_page`, which leaves it be.
        assert!(!asks_to_pair("/pair?token=AB12"));
    }

    #[test]
    fn reading_in_part_is_a_problem_until_the_database_reads() {
        assert!(partial_problem(&Source::Database).is_empty());
        let problem = partial_problem(&Source::Cli("no such table: x".into()));
        assert_eq!(problem.len(), 1);
        assert!(problem[0].contains("no such table: x"));
    }

    fn row(id: &str) -> ThreadRow {
        ThreadRow {
            id: id.into(),
            title: id.into(),
            updated: "2026-11-01T10:00:00.000Z".into(),
            ..ThreadRow::default()
        }
    }

    #[test]
    fn a_page_taking_its_pairing_link_is_not_sent_another() {
        let start = Instant::now();
        let later = |secs| start + Duration::from_secs(secs);
        let mut paired = HashMap::new();
        let mut at = |path: &str, when| pair_turn_at(&mut paired, 1, path, false, when);
        // Opened at a link, as T3 Code 0.1.7 goes on from it.
        assert_eq!(at("/pair?token=x", start), None);
        assert_eq!(at("/pair", start), Some(Pairing::Quiet));
        assert_eq!(at("/", later(1)), None);
        assert_eq!(at("/draft/d", later(1)), None);
        // Reports can come late: the moment at the bare page still isn't
        // asking.
        assert_eq!(at("/pair", later(2)), Some(Pairing::Quiet));
        // A sign-out later is, and starts counting anew.
        assert_eq!(at("/pair", later(40)), Some(Pairing::Send));
        assert_eq!(at("/pair", later(45)), Some(Pairing::Quiet));
    }

    #[test]
    fn a_page_that_keeps_asking_is_given_up_on() {
        let start = Instant::now();
        let later = |secs| start + Duration::from_secs(secs);
        let mut paired = HashMap::new();
        let mut at = |when| pair_turn_at(&mut paired, 1, "/pair", false, when);
        for (i, turn) in [Pairing::Send, Pairing::Send, Pairing::Send, Pairing::GiveUp]
            .into_iter()
            .enumerate()
        {
            assert_eq!(at(later(i as u64 * 31)), Some(turn), "try {i}");
        }
        assert_eq!(at(later(200)), Some(Pairing::Quiet));
        // One being asked for already isn't asked for again.
        let mut other = HashMap::new();
        assert_eq!(
            pair_turn_at(&mut other, 2, "/pair", true, start),
            Some(Pairing::Quiet)
        );
    }

    #[test]
    fn times_are_written_as_t3_code_writes_them() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00");
        // 2026-11-04T11:24:57.489Z, of a run in T3 Code 0.1.7.
        assert_eq!(iso_utc(1_793_791_497), "2026-11-04T11:24:57");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00");
        assert!("2026-11-04T11:24:57.489Z" > iso_utc(1_793_791_497).as_str());
    }

    #[test]
    fn a_drafts_thread_is_the_first_sent_in_its_folder_since() {
        const SINCE: u64 = 1_793_791_497;
        let at = |id: &str, dir: &str, when: &str| ThreadRow {
            project: Some(PathBuf::from(dir)),
            first_run: Some(when.to_owned()),
            ..row(id)
        };
        let dir = Path::new("/r/.features/x");
        let rows = [
            at("older", "/r/.features/x", "2026-11-04T11:24:56.900Z"),
            at("around", "/r", "2026-11-04T11:24:58.000Z"),
            at("later", "/r/.features/x", "2026-11-04T11:25:30.000Z"),
            at("first", "/r/.features/x", "2026-11-04T11:24:59.000Z"),
            ThreadRow {
                child: true,
                ..at("side", "/r/.features/x", "2026-11-04T11:24:58.000Z")
            },
            ThreadRow {
                first_run: None,
                ..at("unsent", "/r/.features/x", "")
            },
        ];
        let found = |taken: &[&str]| drafted_thread(&rows, dir, SINCE, taken).map(|r| r.id.clone());
        assert_eq!(found(&[]).as_deref(), Some("first"));
        // Another stage's thread stays that stage's.
        assert_eq!(found(&["first"]).as_deref(), Some("later"));
        assert_eq!(found(&["first", "later"]), None);
    }

    #[test]
    fn a_row_says_what_its_thread_does() {
        let mut r = row("t1");
        assert_eq!(r.status(), AgentStatus::Idle);
        r.run = Some("running".into());
        assert_eq!(r.status(), AgentStatus::Working);
        r.run = Some("waiting".into());
        assert_eq!(r.status(), AgentStatus::Blocked);
        r.run = Some("completed".into());
        r.completed = Some("2026-11-01T10:05:00.000Z".into());
        assert_eq!(r.status(), AgentStatus::Done, "never looked at");
        r.visited = Some("2026-11-01T10:04:00.000Z".into());
        assert_eq!(r.status(), AgentStatus::Done, "looked at before it ended");
        r.visited = Some("2026-11-01T10:06:00.000Z".into());
        assert_eq!(r.status(), AgentStatus::Idle);
        r.pending = 1;
        assert_eq!(r.status(), AgentStatus::Blocked, "a request waits");
    }

    #[test]
    fn a_row_names_its_agent_as_herdr_does() {
        let mut r = row("t1");
        r.provider = Some("claudeAgent".into());
        assert_eq!(r.agent(), "claude");
        r.provider = Some("codex".into());
        assert_eq!(r.agent(), "codex");
        r.provider = Some("agent".into());
        assert_eq!(r.agent(), "agent");
    }

    #[test]
    fn children_go_on_their_topmost_parents_card() {
        let parent = row("p");
        let mut side = row("s");
        side.parent = Some("p".into());
        side.child = true;
        side.run = Some("running".into());
        let mut sub = row("sub");
        sub.parent = Some("s".into());
        sub.child = true;
        let mut fork = row("f");
        fork.parent = Some("p".into());
        let cards = threads(&[parent, side, sub, fork], true);
        let ids: Vec<&str> = cards.iter().map(|c| c.row.id.as_str()).collect();
        assert_eq!(ids, ["p", "f"]);
        assert_eq!(cards[0].status, AgentStatus::Working, "its side chat works");
        let children: Vec<&str> = cards[0]
            .children
            .iter()
            .map(|(r, _)| r.id.as_str())
            .collect();
        assert_eq!(children, ["s"], "an idle child shows no line");
    }

    #[test]
    fn without_the_server_nothing_works() {
        let mut r = row("t1");
        r.run = Some("running".into());
        assert_eq!(threads(&[r.clone()], false)[0].status, AgentStatus::Idle);
        assert_eq!(threads(&[r], true)[0].status, AgentStatus::Working);
    }

    /// `hold_finished` for thread `t1`, mine, with `seen` looked at in
    /// gerdr and `panes` shown in gerdr's panes.
    fn hold(
        finished: &mut super::Held,
        before: &[T3Thread],
        threads: &mut [T3Thread],
        alive: bool,
        seen: Option<&str>,
        panes: &[&str],
    ) -> bool {
        let mine = HashSet::from(["t1".to_owned()]);
        let in_panes = panes.iter().map(|p| p.to_string()).collect();
        let sight = super::Sight {
            alive,
            seen,
            mine: &mine,
            in_panes: &in_panes,
        };
        super::hold_finished(finished, before, threads, &sight)
    }

    /// Thread `t1` at work, and done as T3 Code's app leaves it while it
    /// shows it: visited as it completed, last opened at `opened`.
    fn working_then_idle(opened: Option<i64>) -> (Vec<T3Thread>, Vec<T3Thread>) {
        let mut r = row("t1");
        r.run = Some("running".into());
        r.opened = opened;
        let working = threads(&[r.clone()], true);
        r.run = Some("completed".into());
        r.completed = Some("2026-11-02T12:17:34.441Z".into());
        r.visited = r.completed.clone();
        (working, threads(&[r], true))
    }

    #[test]
    fn a_thread_finished_out_of_sight_stays_done_until_looked_at() {
        let (working, idle) = working_then_idle(None);
        assert_eq!(idle[0].status, AgentStatus::Idle, "visited as it completed");
        let mut finished = super::Held::new();
        let mut now = idle.clone();
        hold(&mut finished, &working, &mut now, true, None, &[]);
        assert_eq!(now[0].status, AgentStatus::Done, "finished out of sight");
        // Read again unlooked at: still done.
        let mut again = idle.clone();
        hold(&mut finished, &now, &mut again, true, Some("other"), &[]);
        assert_eq!(again[0].status, AgentStatus::Done);
        // Looked at: idle.
        let mut looked = idle.clone();
        hold(&mut finished, &again, &mut looked, true, Some("t1"), &[]);
        assert_eq!(looked[0].status, AgentStatus::Idle);
        assert!(finished.is_empty());
        // Finished in sight, or with the server gone: never held.
        let mut seen = idle.clone();
        hold(&mut finished, &working, &mut seen, true, Some("t1"), &[]);
        assert_eq!(seen[0].status, AgentStatus::Idle);
        let mut gone = idle.clone();
        hold(&mut finished, &working, &mut gone, false, None, &[]);
        assert_eq!(gone[0].status, AgentStatus::Idle);
        // At work again: let go.
        let mut held = idle.clone();
        hold(&mut finished, &working, &mut held, true, None, &[]);
        let mut rerun = working.clone();
        hold(&mut finished, &held, &mut rerun, true, None, &[]);
        assert_eq!(rerun[0].status, AgentStatus::Working);
        assert!(finished.is_empty());
        // Another gerdr's thread: not held here.
        let mut theirs = idle.clone();
        let none = HashSet::new();
        let sight = super::Sight {
            alive: true,
            seen: None,
            mine: &none,
            in_panes: &none,
        };
        assert!(!super::hold_finished(
            &mut finished,
            &working,
            &mut theirs,
            &sight
        ));
        assert_eq!(theirs[0].status, AgentStatus::Idle);
    }

    #[test]
    fn a_held_thread_opened_outside_gerdr_is_let_go() {
        let (working, idle) = working_then_idle(Some(1_000));
        let opened_at = |when: i64| {
            let mut cards = idle.clone();
            cards[0].row.opened = Some(when);
            cards
        };
        let mut finished = super::Held::new();
        let mut now = idle.clone();
        hold(&mut finished, &working, &mut now, true, None, &[]);
        assert_eq!(finished.get("t1"), Some(&Some(1_000)), "as last opened");
        // Opened again by a pane of gerdr's (a reload): still done.
        let mut reload = opened_at(2_000);
        hold(&mut finished, &now, &mut reload, true, None, &["t1"]);
        assert_eq!(reload[0].status, AgentStatus::Done);
        assert_eq!(finished.get("t1"), Some(&Some(2_000)));
        // Opened in T3 Code's own window: let go.
        let mut elsewhere = opened_at(3_000);
        assert!(hold(
            &mut finished,
            &reload,
            &mut elsewhere,
            true,
            None,
            &[]
        ));
        assert_eq!(elsewhere[0].status, AgentStatus::Idle);
        assert!(finished.is_empty());
        // Never opened when held, then opened: let go too.
        let (working, idle) = working_then_idle(None);
        let mut held = idle.clone();
        hold(&mut finished, &working, &mut held, true, None, &[]);
        assert_eq!(finished.get("t1"), Some(&Some(0)));
        let mut opened = idle.clone();
        opened[0].row.opened = Some(5_000);
        hold(&mut finished, &held, &mut opened, true, None, &[]);
        assert_eq!(opened[0].status, AgentStatus::Idle);
    }

    #[test]
    fn a_held_thread_stays_done_across_restarts() {
        let (_, idle) = working_then_idle(Some(1_000));
        // As saved in state.json before gerdr kept when each was opened.
        let mut finished = super::de_held(serde_json::json!(["t1"])).unwrap();
        assert_eq!(finished.get("t1"), Some(&None));
        // Nothing read yet as gerdr starts: nothing forgotten.
        assert!(!hold(&mut finished, &[], &mut [], true, None, &[]));
        assert!(
            finished.contains_key("t1"),
            "kept until the threads are read"
        );
        // The first read: still done, with no card before it, and its open
        // taken as known.
        let mut first = idle.clone();
        assert!(hold(&mut finished, &[], &mut first, false, None, &[]));
        assert_eq!(first[0].status, AgentStatus::Done);
        assert_eq!(finished.get("t1"), Some(&Some(1_000)));
        // As saved now, and read back.
        let json = serde_json::to_value(&finished).unwrap();
        assert_eq!(super::de_held(json).unwrap(), finished);
        // A thread no longer read is forgotten.
        let mut other = threads(&[row("t2")], true);
        assert!(hold(&mut finished, &first, &mut other, true, None, &[]));
        assert!(finished.is_empty());
    }

    #[test]
    fn a_hidden_thread_is_back_once_it_changes() {
        let mut hidden = Hidden::default();
        let mut card = threads(&[row("t1")], true).remove(0);
        hidden.hide(&card);
        assert!(hidden.hides(&card));
        card.row.updated = "2026-11-01T11:00:00.000Z".into();
        assert!(!hidden.hides(&card));
        assert!(hidden.settle(std::slice::from_ref(&card)), "forgotten");
        assert!(hidden.is_empty());
    }

    #[test]
    fn a_thread_closed_while_it_runs_stays_hidden_as_its_run_left_it() {
        let mut hidden = Hidden::default();
        let mut r = row("t1");
        r.run = Some("running".into());
        let card = threads(&[r.clone()], true).remove(0);
        hidden.hide(&card);
        r.updated = "2026-11-01T10:30:00.000Z".into();
        let card = threads(&[r.clone()], true).remove(0);
        assert!(hidden.hides(&card), "hidden while it runs");
        r.run = Some("completed".into());
        r.completed = Some(r.updated.clone());
        let card = threads(&[r.clone()], true).remove(0);
        assert!(hidden.settle(std::slice::from_ref(&card)));
        assert!(hidden.hides(&card), "deleted as the run left it");
        r.updated = "2026-11-01T12:00:00.000Z".into();
        let card = threads(&[r], true).remove(0);
        assert!(!hidden.hides(&card), "back once written to");
    }

    #[test]
    fn links_name_threads_by_environment() {
        let server = Server {
            origin: "http://127.0.0.1:3773".into(),
            pid: 1,
            version: None,
            environment: Some("env-1".into()),
        };
        let link = server.thread_link("http://localhost:3773", "thread:project:p1:t1");
        assert_eq!(
            link,
            "http://localhost:3773/env-1/thread%3Aproject%3Ap1%3At1"
        );
        assert_eq!(
            thread_of_link(&link).as_deref(),
            Some("thread:project:p1:t1")
        );
        assert_eq!(thread_of_link("http://localhost:3773/"), None);
        assert_eq!(
            thread_of_link("http://localhost:3773/settings/general"),
            Some("general".into())
        );
        assert!(is_app(&link, &server));
        assert!(is_app("http://127.0.0.1:3773/pair#token=x", &server));
        assert!(!is_app("http://localhost:3000/", &server));
        assert!(!is_app("https://example.com:3773/", &server));
        assert!(is_app("http://services-api.localhost:3773/", &server));
        assert!(!is_app("http://services-api.localhost:3000/", &server));
        assert!(!is_app("http://localhost.example.com:3773/", &server));
        assert_eq!(origin_of(&link), Some("http://localhost:3773"));
    }

    #[test]
    fn a_pane_stuck_at_the_sign_in_page_is_told_once() {
        let turns: Vec<Pairing> = (0..6).map(pairing_turn).collect();
        assert_eq!(
            turns,
            [
                Pairing::Send,
                Pairing::Send,
                Pairing::Send,
                Pairing::GiveUp,
                Pairing::Quiet,
                Pairing::Quiet
            ]
        );
    }

    #[test]
    fn only_the_sign_in_page_without_a_token_asks_to_pair() {
        assert!(asks_to_pair("/pair"));
        assert!(asks_to_pair("/pair/"));
        assert!(asks_to_pair("/pair?next=%2F"));
        assert!(!asks_to_pair("/pair#token=WQ99"));
        assert!(!asks_to_pair("/"));
        assert!(!asks_to_pair("/pairing-help"));
        assert!(!asks_to_pair("/env/thread"));
    }

    #[test]
    fn the_pairing_link_comes_without_colours() {
        assert_eq!(
            crate::feature::verify::strip_escapes(
                "\u{1b}[1mhttp://127.0.0.1:3773/pair#token=ab\u{1b}[0m"
            ),
            "http://127.0.0.1:3773/pair#token=ab"
        );
    }

    /// A database with the tables read, as T3 Code 0.1.6 makes them (see
    /// `t3_schema.sql`), and a project.
    fn database() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("t3_schema.sql")).unwrap();
        conn.execute_batch(
            "INSERT INTO projection_projects (project_id, title, workspace_root, scripts_json,
                created_at, updated_at) VALUES ('p1', 'app', '/src/app', '[]', '', '');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn projects_are_looked_up_and_an_unreadable_database_says_so() {
        use super::{has_project, project_count};
        let home = std::env::temp_dir().join(format!("gerdr-t3-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        // No database: it can't say, rather than "no project".
        assert!(has_project(&home, Path::new("/src/app")).is_err());
        assert_eq!(project_count(&home), None);
        std::fs::create_dir_all(home.join("userdata")).unwrap();
        let conn = rusqlite::Connection::open(home.join(DB)).unwrap();
        conn.execute_batch(include_str!("t3_schema.sql")).unwrap();
        assert_eq!(project_count(&home), Some(0));
        conn.execute_batch(
            "INSERT INTO projection_projects (project_id, title, workspace_root, scripts_json,
                created_at, updated_at) VALUES ('p1', 'app', '/src/app', '[]', '', '');",
        )
        .unwrap();
        assert_eq!(has_project(&home, Path::new("/src/app")), Ok(true));
        assert_eq!(has_project(&home, Path::new("/src/other")), Ok(false));
        assert_eq!(project_count(&home), Some(1));
        drop(conn);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_filter_not_taken_is_told_in_plain_words() {
        use super::unscoped_words;
        for reason in [
            "no-react",
            "no-groups",
            "no-roots",
            "ignored",
            "overwritten",
            "error",
        ] {
            let words = unscoped_words(reason);
            assert_ne!(words, unscoped_words("something new"), "{reason}");
            assert!(!words.contains(reason) && !words.ends_with('.'), "{reason}");
        }
    }

    #[test]
    fn rows_are_read_from_the_database() {
        let conn = database();
        let now = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";
        let thread = "INSERT INTO orchestration_v2_projection_threads (thread_id, project_id,
            title, default_provider, runtime_mode, interaction_mode, created_at, updated_at,
            archived_at, payload_json, provider_instance_id)";
        let run = "INSERT INTO orchestration_v2_projection_runs (run_id, thread_id, ordinal,
            provider, status, requested_at, completed_at, payload_json)";
        conn.execute_batch(&format!(
            "{thread} VALUES
               ('t1', 'p1', 'Fix it', 'c', 'm', 'i', '', {now}, NULL,
                '{{\"worktreePath\": \"/src/app-wt\", \"branch\": \"t3/fix\",
                   \"modelSelection\": {{\"model\": \"opus\"}}, \"lastVisitedAt\": null,
                   \"lineage\": {{\"parentThreadId\": null}}}}', 'claudeAgent'),
               ('old', 'p1', 'Old', 'c', 'm', 'i', '', '2020-01-01T00:00:00.000Z', NULL,
                '{{}}', 'codex'),
               ('gone', 'p1', 'Gone', 'c', 'm', 'i', '', {now}, {now}, '{{}}', 'codex'),
               ('side', 'p1', 'Aside', 'c', 'm', 'i', '', {now}, NULL,
                '{{\"lineage\": {{\"parentThreadId\": \"t1\", \"relationshipToParent\": \"fork\",
                   \"sideChat\": true}}}}', 'claudeAgent');
             {run} VALUES
               ('r1', 't1', 1, 'claudeAgent', 'completed', '', '2020-01-01T00:00:00.000Z', '{{}}'),
               ('r2', 't1', 2, 'claudeAgent', 'running', '', NULL, '{{}}');
             INSERT INTO orchestration_v2_projection_runtime_requests (runtime_request_id,
               thread_id, node_id, kind, status, created_at, payload_json)
               VALUES ('q1', 'side', 'n', 'approval', 'pending', '', '{{}}');"
        ))
        .unwrap();
        let rows = read_rows(&conn, &[]).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["t1", "side"]);
        // One held done is read however old, but not once archived.
        let held = read_rows(&conn, &["old".into(), "gone".into()]).unwrap();
        let ids: Vec<&str> = held.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["t1", "old", "side"]);
        let t1 = &rows[0];
        assert_eq!(t1.status(), AgentStatus::Working, "by its latest run");
        assert_eq!(
            t1.paths(),
            [PathBuf::from("/src/app-wt"), PathBuf::from("/src/app")]
        );
        assert_eq!(t1.model.as_deref(), Some("opus"));
        assert_eq!(t1.agent(), "claude");
        assert!(rows[1].child);
        assert_eq!(rows[1].status(), AgentStatus::Blocked);
        assert_eq!(migration(&conn), Some(76));
    }

    #[test]
    fn a_changed_table_fails_the_read_instead_of_misreading_it() {
        let conn = database();
        conn.execute_batch(
            "ALTER TABLE orchestration_v2_projection_runs RENAME COLUMN status TO state;",
        )
        .unwrap();
        let err = read_rows(&conn, &[]).unwrap_err().to_string();
        assert!(err.contains("status"), "{err}");
    }

    #[test]
    fn the_server_lists_threads_with_basic_status() {
        let json = br#"[
            {"threadId":"a","title":"Fix","status":"running","providerInstanceId":"claudeAgent",
             "model":"opus","settled":false,"parentThreadId":null,"updatedAt":"2026-11-01T21:07:20Z",
             "itemCount":3},
            {"threadId":"b","title":"Done","status":"completed","settled":true},
            {"threadId":"c","title":"Sub","status":"waiting","parentThreadId":"a",
             "relationshipToParent":"subagent"},
            {"threadId":"d","status":"idle"}
        ]"#;
        let rows = cli_threads(json, Path::new("/src/app")).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "c", "d"], "none settled");
        assert_eq!(rows[0].status(), AgentStatus::Working);
        assert_eq!(rows[0].paths(), [PathBuf::from("/src/app")]);
        assert_eq!(rows[0].agent(), "claude");
        assert!(rows[1].child);
        assert_eq!(rows[1].status(), AgentStatus::Blocked);
        assert_eq!(rows[2].status(), AgentStatus::Idle);
    }

    #[test]
    fn the_server_starts_at_the_version_pinned() {
        let none = |_: &str| false;
        let start = |pin, port| Start {
            home: None,
            pin,
            port,
        };
        assert_eq!(start(None, None).command(none), "npx t3@latest serve");
        assert_eq!(start(None, None).command(|p| p == "t3"), "t3 serve");
        assert_eq!(
            start(Some("0.1.6"), Some(3773)).command(none),
            "npx --yes t3@0.1.6 serve --port 3773"
        );
        let home = std::env::temp_dir().join(format!("gerdr-t3-start-{}", std::process::id()));
        let dir = home.join("runtime/versions/0.1.6");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t3"), "").unwrap();
        std::fs::write(dir.join(".install-complete"), "0.1.6\n").unwrap();
        let pinned = Start {
            home: Some(&home),
            pin: Some("0.1.6"),
            port: None,
        };
        let command = pinned.command(none);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(
            command,
            format!(
                "{} serve --base-dir {}",
                dir.join("t3").display(),
                home.display()
            )
        );
    }

    /// The running server's threads of this folder's project, as gerdr asks
    /// while the database can't be read. Needs T3 Code running here; run by
    /// hand with `T3_PROJECT=<folder> cargo test -- --ignored running_server`.
    #[test]
    #[ignore = "asks the T3 Code server running here"]
    fn the_running_server_lists_this_folder() {
        let home = home().expect("T3 Code's home");
        // A folder of a project of T3 Code's: `$T3_PROJECT`, else this one.
        let dir = std::env::var_os("T3_PROJECT")
            .map_or_else(|| std::env::current_dir().unwrap(), PathBuf::from);
        let rows = cli_rows(&home, &[dir]);
        for row in &rows {
            eprintln!("{} {:?} {:?} {}", row.id, row.run, row.status(), row.title);
        }
        assert!(!rows.is_empty());
    }

    #[test]
    fn each_folder_has_a_host_of_its_own() {
        use super::app_host;
        let host = |dir: &str| app_host(std::path::Path::new(dir));
        // The folder's name, then its path's hash.
        let api = host("/w/ao-app/services-api");
        assert!(api.starts_with("services-api-") && api.ends_with(".localhost"));
        assert!(host("/w/ao-app/aft_backend").starts_with("aft-backend-"));
        assert!(host("/w/ao-app").starts_with("ao-app-"));
        assert!(host("/home/Me/My Repo!").starts_with("my-repo-"));
        assert!(host("/").starts_with("root-"));
        assert!(host("/w/__").starts_with("root-"));
        // The same folder, the same host, run after run.
        assert_eq!(host("/w/front/app"), host("/w/front/app"));
        assert_eq!(host("/w/front/app"), "app-dcc5975d.localhost");
        // Folders of one name apart.
        assert_ne!(host("/w/front/app"), host("/w/back/app"));
        // One DNS label of letters, digits and hyphens.
        for dir in ["/w/ao-app", "/", &format!("/w/{}", "a-".repeat(80))] {
            let h = host(dir);
            let label = h.strip_suffix(".localhost").unwrap();
            assert!(label.len() <= 63, "{h}");
            assert!(!label.starts_with('-') && !label.ends_with('-'), "{h}");
            assert!(
                label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            );
        }
    }

    #[test]
    fn a_link_moves_to_another_origin() {
        use super::at_origin;
        let link = "http://localhost:3773/pair#token=abc";
        let moved = at_origin(link, "http://api.localhost:3773");
        assert_eq!(moved, "http://api.localhost:3773/pair#token=abc");
        assert_eq!(at_origin("not a link", "http://a:1"), "not a link");
    }
}
