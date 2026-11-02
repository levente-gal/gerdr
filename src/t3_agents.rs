//! T3 Code's threads as agents (see `agents`). T3 Code (t3.codes) runs
//! coding agents (Claude, Codex, Cursor, OpenCode, …) headless in a server
//! of its own and shows them in a web app that server serves; gerdr shows
//! that app in a web pane like any other page. The server keeps its
//! threads in a SQLite database under its home (`~/.t3`, or
//! `$T3CODE_HOME`), which a thread reads every `DB_INTERVAL`, read only:
//! each thread's latest run says whether it works, waits for the user or
//! finished since the user last looked at it (`lastVisitedAt`, which T3
//! Code sets itself when its app shows the thread). Where the server runs
//! is in `userdata/server-runtime.json`.
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
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
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
/// workspaces in one folder share one.
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
    if label.is_empty() {
        "root.localhost".to_owned()
    } else {
        format!("{label}.localhost")
    }
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
/// the last day, at work, waiting, or done since the user last looked;
/// none archived, deleted or settled.
pub fn read_rows(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<ThreadRow>> {
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
              WHERE q.thread_id = t.thread_id AND q.status = 'pending')
        FROM orchestration_v2_projection_threads t
        LEFT JOIN projection_projects p ON p.project_id = t.project_id
        LEFT JOIN orchestration_v2_projection_runs r ON r.run_id = (
            SELECT run_id FROM orchestration_v2_projection_runs
            WHERE thread_id = t.thread_id ORDER BY ordinal DESC LIMIT 1)
        WHERE t.archived_at IS NULL AND t.deleted_at IS NULL
          AND json_extract(t.payload_json, '$.settledAt') IS NULL
          AND (t.updated_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day')
               OR r.status IN ('preparing', 'queued', 'starting', 'running', 'waiting')
               OR r.completed_at > coalesce(json_extract(t.payload_json, '$.lastVisitedAt'), ''))",
    )?;
    let rows = query.query_map([], |r| {
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
                    Some(c) => match read_rows(c) {
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
                        && let Ok(rows) = read_rows(c)
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
        .map(|link| strip_escapes(link.trim()))
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

/// Whether T3 Code at `home` has a project at folder `dir`; `None` when
/// its database can't say.
fn has_project(home: &Path, dir: &Path) -> Option<bool> {
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(home.join(DB), flags).ok()?;
    let found = conn.query_row(
        "SELECT 1 FROM projection_projects WHERE workspace_root = ?1 AND deleted_at IS NULL",
        [dir.to_string_lossy()],
        |_| Ok(()),
    );
    match found {
        Ok(()) => Some(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Some(false),
        Err(err) => {
            tracing::warn!("t3 code: looking for project {}: {err}", dir.display());
            None
        }
    }
}

/// Add folder `dir` as a T3 Code project of `server` (by `t3 project add`).
/// How long `t3 project add` may take.
const ADD_PROJECT_TIMEOUT: Duration = Duration::from_secs(30);

fn add_project(home: &Path, server: &Server, dir: &Path) -> Result<(), String> {
    let binary = t3_binary(home, server).ok_or_else(|| "no t3 command found".to_owned())?;
    use std::process::Stdio;
    let mut child = std::process::Command::new(&binary)
        .args(["project", "add", "--base-dir"])
        .arg(home)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{}: {err}", binary.display()))?;
    // It only talks to the server on this machine: one stuck is stopped.
    let until = Instant::now() + ADD_PROJECT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer in {}s", ADD_PROJECT_TIMEOUT.as_secs()));
            }
            Err(err) => return Err(err.to_string()),
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut err = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
    }
    Err(err.lines().last().unwrap_or("failed").trim().to_owned())
}

/// `text` without terminal escape sequences.
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: up to its final letter.
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
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
    /// Whether a pairing link is being asked for.
    pairing: bool,
    /// The web panes the extension was told the folders of, for T3 Code's
    /// project filter (see `Gerdr::scope_t3`).
    scoped: HashSet<u32>,
    /// The folders looked at for T3 Code projects, once each.
    checked: HashSet<PathBuf>,
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
        watch_db(home.clone(), self.t3.dirs.clone(), move |s| {
            tx.send(s).is_ok()
        });
        self.watch_t3_updates();
        let tick = move |_, _: &mut (), state: &mut crate::Gerdr| {
            state.t3_dirs();
            let server = Server::read(&home);
            let alive = server.as_ref().is_some_and(Server::alive);
            if server != state.t3.server || alive != state.t3.alive {
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
        if self.projects.t3_hidden.settle(&all) {
            self.projects.save();
        }
        let hidden = &self.projects.t3_hidden;
        self.t3.threads = all.into_iter().filter(|t| !hidden.hides(t)).collect();
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
                worktrees: self.git.get(&ws.dir).map_or_else(Vec::new, |git| {
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
                    seen: self.host_focused && shown.as_deref() == Some(&thread.row.id),
                    finished: None,
                })
            })
            .collect()
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

    /// Keep web pane `pane`, showing T3 Code's app, to its workspace's
    /// projects: told to the extension once per pane, which applies it
    /// at each load.
    fn scope_t3(&mut self, pane: u32) {
        if self.t3.scoped.contains(&pane) {
            return;
        }
        let Some(w) = self
            .workspaces
            .iter()
            .position(|ws| ws.tabs.iter().any(|t| t.panes.iter().any(|p| p.id == pane)))
        else {
            return;
        };
        let dirs = self.t3_scope(w);
        if self.scope_web_pane(pane, &dirs) {
            self.t3.scoped.insert(pane);
        }
    }

    /// T3 Code's app in web pane `pane` didn't take its project filter,
    /// for `reason` (see the extension's `scopeT3`): logged in full, for
    /// whoever fixes gerdr for the T3 Code version that changed it, and
    /// told once per reason.
    pub fn t3_unscoped(&mut self, pane: u32, reason: &str, detail: &str) {
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
            "t3 code",
            format!(
                "T3 Code {version} doesn't take gerdr's project filter ({reason}): see the log"
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
        let (Some(home), Some(ws)) = (self.t3.home.clone(), self.workspaces.get(w)) else {
            return;
        };
        let dir = ws.dir.clone();
        if !dir.join(".git").exists() || Some(dir.as_path()) == crate::home_dir().as_deref() {
            return;
        }
        if !self.t3.checked.insert(dir.clone()) {
            return;
        }
        use smithay::reexports::calloop::channel;
        let (tx, rx) = channel::channel::<String>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(text) = event {
                let open_log = ("Open log".to_owned(), crate::menu::MenuAction::OpenLog);
                state.toast_with(text, crate::toast::Level::Error, Some(open_log));
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("t3 code: {err}");
        }
        let server = server.clone();
        std::thread::spawn(move || {
            if has_project(&home, &dir) == Some(false)
                && let Err(err) = add_project(&home, &server, &dir)
            {
                tracing::warn!("t3 code: adding {}: {err}", dir.display());
                let name = dir.file_name().unwrap_or_default().to_string_lossy();
                let _ = tx.send(format!("Couldn't add {name} to T3 Code's projects: {err}"));
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
        use crate::toast::Level;
        let Some(server) = self.t3.server.clone().filter(|_| self.t3.alive) else {
            let action = ("Start".to_owned(), crate::menu::MenuAction::StartT3);
            let text = "T3 Code's server isn't running".to_owned();
            return self.toast_with(text, Level::Warning, Some(action));
        };
        let link = |origin: &str| match thread {
            Some(id) => server.thread_link(origin, id),
            None => format!("{origin}/"),
        };
        let w = w.min(self.workspaces.len().saturating_sub(1));
        self.add_t3_project(w, &server);
        let own = self.t3_origin(w, &server);
        if let Some((t, pane, url)) = self.t3_pane_in(w) {
            self.show_leaf(w, t, pane);
            // One at another host (opened before workspaces had their
            // own) moves to the workspace's: its filter is its own there.
            let moves = origin_of(&url) != Some(own.as_str());
            if moves && thread.is_none() {
                let target = at_origin(&url, &own);
                self.navigate_web_pane(pane, &target);
                return;
            }
            let target = link(&own);
            if thread.is_some() && url != target && !self.navigate_web_pane(pane, &target) {
                // Without gerdr's extension in its browser it can't be
                // told where to go: a tab of its own then.
                self.launch(&target, crate::launch::Place::Tab);
            }
            return;
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
        if self.t3.pairing {
            return;
        }
        let (tx, rx) = channel::channel::<Result<String, String>>();
        let inserted = self.loop_handle.insert_source(rx, move |event, _, state| {
            let channel::Event::Msg(result) = event else {
                return;
            };
            state.t3.pairing = false;
            match result {
                Ok(pairing) => {
                    // At the app's host: `t3 pair` names its own.
                    let pairing = at_origin(&pairing, &origin);
                    if let Some(marker) = &marker
                        && let Err(err) = std::fs::write(marker, &origin)
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
        self.t3.pairing = true;
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
        if asks_to_pair(path) {
            // Signed out (a browser never paired, or the server forgot
            // it): sign it in, whichever way it got here.
            let now = Instant::now();
            let sent = self.t3.paired_panes.get(&pane).copied();
            let recent = sent.is_some_and(|(at, _)| now.duration_since(at) < PAIR_AGAIN);
            if recent || self.t3.pairing {
                return;
            }
            let tries = sent.map_or(0, |(_, tries)| tries);
            match pairing_turn(tries) {
                Pairing::Send => {
                    self.t3.paired_panes.insert(pane, (now, tries + 1));
                    let server = server.clone();
                    let origin = origin.to_owned();
                    let marker = self.t3_paired_marker(&origin);
                    self.pair_t3(server, origin, None, marker, Some(pane));
                }
                Pairing::GiveUp => {
                    self.t3.paired_panes.insert(pane, (now, tries + 1));
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
        // Past the sign-in page: signed in, so a later sign-out tries again.
        if !path.starts_with("/pair") {
            self.t3.paired_panes.remove(&pane);
            self.scope_t3(pane);
        }
        if self.t3.pending.is_none() || path.starts_with("/pair") {
            return;
        }
        if let Some(then) = self.t3.pending.take() {
            // At the host it signed in at.
            let then_origin = origin_of(&then).unwrap_or_default().to_owned();
            let target = format!("{origin}{}", &then[then_origin.len()..]);
            if target != url {
                self.navigate_web_pane(pane, &target);
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
            strip_escapes("\u{1b}[1mhttp://127.0.0.1:3773/pair#token=ab\u{1b}[0m"),
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
        let rows = read_rows(&conn).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["t1", "side"]);
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
        let err = read_rows(&conn).unwrap_err().to_string();
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
        assert_eq!(host("/w/ao-app/services-api"), "services-api.localhost");
        assert_eq!(host("/w/ao-app/aft_backend"), "aft-backend.localhost");
        assert_eq!(host("/w/ao-app"), "ao-app.localhost");
        assert_eq!(host("/home/Me/My Repo!"), "my-repo.localhost");
        assert_eq!(host("/"), "root.localhost");
        assert_eq!(host("/w/__"), "root.localhost");
        assert!(host(&format!("/w/{}", "a".repeat(80))).len() <= 50 + ".localhost".len());
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
