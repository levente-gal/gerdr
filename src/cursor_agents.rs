//! Cursor's agent chats as agents (see `agents`): what each is doing, read
//! from two places. Cursor's hooks (`~/.cursor/hooks.json`, which gerdr
//! adds itself to) say as soon as a chat starts or stops, through
//! `gerdr cursor-hook`, which hands each event to the gerdr Cursor runs in
//! over a socket of its own. Cursor keeps its chats in its profile's
//! database, which a thread reads every `DB_INTERVAL`: their names, whether
//! one has news the user hasn't read or waits for the user, and which repo
//! it is in. Cursor writes there lazily: a chat's wait for approval shows
//! some seconds late, and a new chat only once its first run stops, so
//! whether one runs goes by the hooks. No hook says that a chat waits.

use std::{
    collections::HashMap,
    io::Read,
    os::unix::net::UnixDatagram,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::herdr::AgentStatus;

/// How often the database is read.
pub const DB_INTERVAL: Duration = Duration::from_secs(1);
/// How long a missing database is waited for before it is looked for
/// again.
const DB_RETRY: Duration = Duration::from_secs(5);
/// How far back a chat's last change may be for it to be read at all,
/// unless it waits or has news.
const RECENT_MS: i64 = 24 * 60 * 60 * 1000;
/// How long a hook's word that a run ended holds over an older database
/// row: then the database knows better (the user read the chat, say).
pub const HOOK_HOLD_MS: i64 = 30_000;
/// How long a hook's word that a chat runs holds: one tool can take long
/// without a hook between, and a run that dies says nothing.
pub const WORKING_HOLD_MS: i64 = 30 * 60_000;
/// The most of a prompt a chat the database doesn't know yet is named by.
const PROMPT_CHARS: usize = 80;
/// The subcommand Cursor's hooks run.
pub const HOOK_COMMAND: &str = "cursor-hook";
/// The hook events gerdr listens to.
const EVENTS: [&str; 8] = [
    "sessionStart",
    "beforeSubmitPrompt",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "afterAgentResponse",
    "stop",
    "sessionEnd",
];
/// How long a hook may take, in seconds: it only passes a datagram on.
const HOOK_TIMEOUT: u64 = 2;
/// The most of a hook's input read.
const MAX_INPUT: u64 = 4 << 20;

/// The time now, in milliseconds since the epoch, as Cursor keeps it.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A chat as the database has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatRow {
    pub id: String,
    /// When it last changed, in milliseconds since the epoch.
    pub updated_ms: i64,
    pub name: Option<String>,
    pub subtitle: Option<String>,
    /// It waits for the user to approve something or answer it.
    pub blocking: bool,
    /// It has news the user hasn't read.
    pub unread: bool,
    /// Its run's state: `generating` while it runs, else `completed`,
    /// `aborted`, `error` or `none`.
    pub status: Option<String>,
    pub context_pct: Option<f64>,
    pub files: Option<i64>,
    pub added: Option<i64>,
    pub removed: Option<i64>,
    /// The folders it works in, the likeliest first: its own (a worktree,
    /// maybe), the repo that one came from, the repos it touched.
    pub paths: Vec<PathBuf>,
    /// It runs in a worktree of its own.
    pub worktree: bool,
}

impl ChatRow {
    /// What the row alone says it is doing.
    pub fn status(&self) -> AgentStatus {
        if self.blocking {
            AgentStatus::Blocked
        } else if self.status.as_deref() == Some("generating") {
            AgentStatus::Working
        } else if self.unread {
            AgentStatus::Done
        } else {
            AgentStatus::Idle
        }
    }
}

/// What a hook said about a chat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HookEvent {
    /// The hook's event, e.g. `stop`.
    pub event: String,
    /// The chat's id (Cursor's conversation id).
    pub chat: String,
    /// How its run ended, for `stop`.
    #[serde(default)]
    pub status: Option<String>,
    /// The model it runs.
    #[serde(default)]
    pub model: Option<String>,
    /// The folders it works in.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
    /// What the user asked it, cut short, for `beforeSubmitPrompt`.
    #[serde(default)]
    pub prompt: Option<String>,
    /// When the hook ran, in milliseconds since the epoch.
    pub at_ms: i64,
}

impl HookEvent {
    /// The event of hook input `payload`, which ran `at_ms`; `None` without
    /// an event or a chat.
    pub fn from_payload(payload: &Value, at_ms: i64) -> Option<Self> {
        let text = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
        let roots = payload
            .get("workspace_roots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect();
        let prompt = text("prompt")
            .and_then(|p| Some(p.lines().map(str::trim).find(|l| !l.is_empty())?.to_owned()))
            .map(|line| match line.char_indices().nth(PROMPT_CHARS) {
                Some((cut, _)) => format!("{}…", &line[..cut]),
                None => line,
            });
        Some(Self {
            event: text("hook_event_name")?,
            chat: text("conversation_id").or_else(|| text("session_id"))?,
            status: text("status"),
            model: text("model").filter(|m| !m.is_empty()),
            roots,
            prompt,
            at_ms,
        })
    }

    /// Whether its word still holds at `now_ms`: that a chat runs, for
    /// `WORKING_HOLD_MS`; anything else, for `HOOK_HOLD_MS`.
    fn holds(&self, now_ms: i64) -> bool {
        let hold = match self.status() {
            Some(AgentStatus::Working) => WORKING_HOLD_MS,
            _ => HOOK_HOLD_MS,
        };
        now_ms - self.at_ms <= hold
    }

    /// What it says the chat is doing; `None` for an event that says
    /// nothing of it.
    pub fn status(&self) -> Option<AgentStatus> {
        match self.event.as_str() {
            "sessionStart" | "beforeSubmitPrompt" | "preToolUse" | "postToolUse"
            | "postToolUseFailure" | "afterAgentResponse" => Some(AgentStatus::Working),
            "stop" => Some(match self.status.as_deref() {
                Some("aborted") => AgentStatus::Idle,
                _ => AgentStatus::Done,
            }),
            _ => None,
        }
    }
}

/// What chat `row` is doing, given what a hook said of it last: a hook's
/// word holds while it is newer than the row and not too old (see
/// `HookEvent::holds`). Only the row knows whether it waits for the user.
pub fn merged_status(row: &ChatRow, hook: Option<&HookEvent>, now_ms: i64) -> AgentStatus {
    let from_row = row.status();
    if from_row == AgentStatus::Blocked {
        return from_row;
    }
    hook.filter(|h| h.at_ms > row.updated_ms && h.holds(now_ms))
        .and_then(HookEvent::status)
        .unwrap_or(from_row)
}

/// A chat as cards show it.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorChat {
    pub row: ChatRow,
    pub status: AgentStatus,
    pub model: Option<String>,
}

/// What gerdr knows of Cursor's chats: the database's rows and what the
/// hooks said last of each.
#[derive(Default)]
pub struct Chats {
    pub rows: HashMap<String, ChatRow>,
    pub hooks: HashMap<String, HookEvent>,
}

impl Chats {
    /// Take in the rows of a fresh read.
    pub fn set_rows(&mut self, rows: Vec<ChatRow>) {
        self.rows = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
    }

    /// Take in what a hook said, keeping what earlier ones said of the
    /// chat's folders, prompt and model when it doesn't; a session's end
    /// forgets its chat's.
    pub fn hear(&mut self, mut event: HookEvent) {
        if event.event == "sessionEnd" {
            self.hooks.remove(&event.chat);
            return;
        }
        if let Some(before) = self.hooks.remove(&event.chat) {
            if event.roots.is_empty() {
                event.roots = before.roots;
            }
            event.prompt = event.prompt.or(before.prompt);
            event.model = event.model.or(before.model);
        }
        self.hooks.insert(event.chat.clone(), event);
    }

    /// The chats doing something or with news at `now_ms`, the most
    /// recently changed first: those of the database, and those only
    /// hooks know of yet (a first run), named by their prompt. What hooks
    /// said long ago is forgotten.
    pub fn live(&mut self, now_ms: i64) -> Vec<CursorChat> {
        self.hooks.retain(|_, h| h.holds(now_ms));
        let unwritten = self
            .hooks
            .values()
            .filter(|h| !self.rows.contains_key(&h.chat))
            .map(|h| ChatRow {
                id: h.chat.clone(),
                name: h.prompt.clone(),
                paths: h.roots.clone(),
                ..ChatRow::default()
            });
        let rows: Vec<ChatRow> = self.rows.values().cloned().chain(unwritten).collect();
        let mut chats: Vec<CursorChat> = rows
            .into_iter()
            .map(|mut row| {
                let hook = self.hooks.get(&row.id);
                if row.paths.is_empty() {
                    row.paths = hook.map(|h| h.roots.clone()).unwrap_or_default();
                }
                CursorChat {
                    status: merged_status(&row, hook, now_ms),
                    model: hook.and_then(|h| h.model.clone()),
                    row,
                }
            })
            .filter(|c| c.status != AgentStatus::Idle)
            .collect();
        chats.sort_by_key(|c| std::cmp::Reverse(c.row.updated_ms));
        chats
    }
}

/// A workspace as placing a chat sees it: its folder and whether it is
/// the project's Cursor workspace.
pub struct Spot<'a> {
    pub dir: &'a Path,
    pub cursor: bool,
}

/// Which of `spots` a chat working in `paths` goes under: the one whose
/// folder holds the likeliest path deepest (one in the home folder holds
/// everything, so it doesn't count), else the Cursor workspace, if the
/// path is in one of `project_dirs`; `None` for a chat of another project.
pub fn place(
    paths: &[PathBuf],
    spots: &[Spot],
    project_dirs: &[&Path],
    home: Option<&Path>,
) -> Option<usize> {
    for path in paths {
        let holding = spots
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.cursor && Some(s.dir) != home && path.starts_with(s.dir))
            .max_by_key(|(_, s)| s.dir.components().count());
        if let Some((i, _)) = holding {
            return Some(i);
        }
    }
    let ours = paths.iter().any(|path| {
        project_dirs
            .iter()
            .any(|dir| Some(*dir) != home && path.starts_with(dir))
    });
    ours.then(|| spots.iter().position(|s| s.cursor)).flatten()
}

/// The chats read from the database at `conn`, given when each was read
/// last and its run's state then (`known`, brought up to date): those
/// changed in the last `RECENT_MS` before `now_ms`, waiting or with news.
pub fn read_rows(
    conn: &rusqlite::Connection,
    now_ms: i64,
    known: &mut HashMap<String, (i64, Option<String>)>,
) -> rusqlite::Result<Vec<ChatRow>> {
    let mut headers = conn.prepare_cached(
        "SELECT composerId, lastUpdatedAt,
            json_extract(value, '$.name'), json_extract(value, '$.subtitle'),
            json_extract(value, '$.hasBlockingPendingActions'),
            json_extract(value, '$.hasUnreadMessages'),
            json_extract(value, '$.contextUsagePercent'),
            json_extract(value, '$.filesChangedCount'),
            json_extract(value, '$.totalLinesAdded'),
            json_extract(value, '$.totalLinesRemoved'),
            json_extract(value, '$.agentLocation.type'),
            json_extract(value, '$.agentLocation.environment.uri.fsPath'),
            json_extract(value, '$.agentLocation.sourceRepoRootPath'),
            json_extract(value, '$.trackedGitRepos')
        FROM composerHeaders
        WHERE coalesce(isArchived, 0) = 0 AND coalesce(isSubagent, 0) = 0
          AND (lastUpdatedAt > ?1
               OR json_extract(value, '$.hasBlockingPendingActions')
               OR json_extract(value, '$.hasUnreadMessages'))",
    )?;
    let mut rows: Vec<ChatRow> = headers
        .query_map([now_ms - RECENT_MS], |r| {
            let flag = |i: usize| r.get::<_, Option<i64>>(i).map(|v| v.unwrap_or(0) != 0);
            let mut paths: Vec<PathBuf> = [r.get::<_, Option<String>>(11)?, r.get(12)?]
                .into_iter()
                .flatten()
                .map(PathBuf::from)
                .collect();
            let repos: Option<String> = r.get(13)?;
            paths.extend(repo_paths(repos.as_deref()));
            Ok(ChatRow {
                id: r.get(0)?,
                updated_ms: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                name: r.get(2)?,
                subtitle: r.get(3)?,
                blocking: flag(4)?,
                unread: flag(5)?,
                status: None,
                context_pct: r.get(6)?,
                files: r.get(7)?,
                added: r.get(8)?,
                removed: r.get(9)?,
                worktree: r.get::<_, Option<String>>(10)?.as_deref() == Some("worktree"),
                paths,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut status = conn.prepare_cached(
        "SELECT json_extract(value, '$.status') FROM cursorDiskKV WHERE key = ?1",
    )?;
    for row in &mut rows {
        // A run's state is read again only once its chat changed, or while
        // it runs.
        let cached = known
            .get(&row.id)
            .filter(|(at, s)| *at == row.updated_ms && s.as_deref() != Some("generating"));
        row.status = match cached {
            Some((_, s)) => s.clone(),
            None => {
                let key = format!("composerData:{}", row.id);
                let mut found = status.query_map([key], |r| r.get::<_, Option<String>>(0))?;
                found.next().transpose()?.flatten()
            }
        };
        known.insert(row.id.clone(), (row.updated_ms, row.status.clone()));
    }
    known.retain(|id, _| rows.iter().any(|r| r.id == *id));
    Ok(rows)
}

/// The repos in `trackedGitRepos`, a JSON list of `{repoPath, …}`.
fn repo_paths(json: Option<&str>) -> Vec<PathBuf> {
    let Some(Ok(Value::Array(repos))) = json.map(serde_json::from_str::<Value>) else {
        return Vec::new();
    };
    repos
        .iter()
        .filter_map(|r| r.get("repoPath")?.as_str())
        .map(PathBuf::from)
        .collect()
}

/// Read the chats in the database at `db` every `DB_INTERVAL`, on a thread
/// of its own, and hand them to `send` whenever they changed. A message on
/// the returned sender asks for a read now. Stops once `send` returns false
/// or the sender is dropped.
pub fn watch_db(
    db: PathBuf,
    mut send: impl FnMut(Vec<ChatRow>) -> bool + Send + 'static,
) -> mpsc::Sender<()> {
    let (poke, poked) = mpsc::channel();
    std::thread::spawn(move || {
        let mut conn: Option<rusqlite::Connection> = None;
        let mut version = None;
        let mut known = HashMap::new();
        let mut last: Option<Vec<ChatRow>> = None;
        loop {
            if conn.is_none() && db.exists() {
                conn = open_read_only(&db)
                    .inspect_err(|err| tracing::debug!("cursor's database: {err}"))
                    .ok();
                version = None;
            }
            let wait = match &conn {
                None => DB_RETRY,
                Some(c) => {
                    match read_if_changed(c, &mut version, &mut known) {
                        Ok(Some(rows)) if last.as_ref() != Some(&rows) => {
                            if !send(rows.clone()) {
                                return;
                            }
                            last = Some(rows);
                        }
                        Ok(_) => {}
                        Err(err) => {
                            tracing::debug!("cursor's database: {err}");
                            if !matches!(
                                err.sqlite_error_code(),
                                Some(rusqlite::ErrorCode::DatabaseBusy)
                            ) {
                                conn = None;
                            }
                        }
                    }
                    DB_INTERVAL
                }
            };
            match poked.recv_timeout(wait) {
                Ok(()) => while poked.try_recv().is_ok() {},
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    poke
}

/// The database at `db`, opened to read only: Cursor writes it meanwhile.
fn open_read_only(db: &Path) -> rusqlite::Result<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(db, flags)?;
    conn.busy_timeout(Duration::from_millis(100))?;
    Ok(conn)
}

/// The chats in `conn`, unless nothing was written since `version`.
fn read_if_changed(
    conn: &rusqlite::Connection,
    version: &mut Option<i64>,
    known: &mut HashMap<String, (i64, Option<String>)>,
) -> rusqlite::Result<Option<Vec<ChatRow>>> {
    let now: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
    // A chat that runs is read again anyway: its state may lag its row.
    let running = known
        .values()
        .any(|(_, s)| s.as_deref() == Some("generating"));
    if *version == Some(now) && !running {
        return Ok(None);
    }
    let rows = read_rows(conn, now_ms(), known)?;
    *version = Some(now);
    Ok(Some(rows))
}

/// Where gerdr on Wayland socket `socket_name` hears hooks:
/// `$XDG_RUNTIME_DIR/<socket_name>.cursor`, "gerdr-" first unless the name
/// starts with "gerdr" (as `run_hook` looks for them).
pub fn hook_socket(socket_name: &std::ffi::OsStr) -> Option<PathBuf> {
    let socket_name = socket_name.to_string_lossy();
    let name = if socket_name.starts_with("gerdr") {
        format!("{socket_name}.cursor")
    } else {
        format!("gerdr-{socket_name}.cursor")
    };
    Some(runtime_dir()?.join(name))
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
}

/// Listen for hooks at `path`, made anew, without blocking. The sockets
/// gerdrs that are gone left next to it go.
pub fn bind_hook_socket(path: &Path) -> std::io::Result<UnixDatagram> {
    let _ = std::fs::remove_file(path);
    let socket = UnixDatagram::bind(path)?;
    socket.set_nonblocking(true)?;
    if let Some(dir) = path.parent() {
        remove_dead_sockets(dir);
    }
    Ok(socket)
}

/// Remove the hook sockets in `dir` no gerdr listens on.
fn remove_dead_sockets(dir: &Path) {
    let (Ok(probe), Ok(entries)) = (UnixDatagram::unbound(), std::fs::read_dir(dir)) else {
        return;
    };
    let _ = probe.set_nonblocking(true);
    for path in entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_hook_socket(p))
    {
        // An empty datagram, which a listening gerdr passes over.
        if let Err(err) = probe.send_to(&[], &path)
            && err.kind() == std::io::ErrorKind::ConnectionRefused
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether `path` is named as a gerdr's hook socket.
fn is_hook_socket(path: &Path) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy())
        .is_some_and(|n| n.starts_with("gerdr") && n.ends_with(".cursor"))
}

/// The events waiting on `socket`.
pub fn take_events(socket: &UnixDatagram) -> Vec<HookEvent> {
    let mut events = Vec::new();
    let mut buf = vec![0u8; 64 << 10];
    while let Ok(n) = socket.recv(&mut buf) {
        if n == 0 {
            continue;
        }
        match serde_json::from_slice(&buf[..n]) {
            Ok(event) => events.push(event),
            Err(err) => tracing::debug!("cursor hook: {err}"),
        }
    }
    events
}

/// `gerdr cursor-hook`: read a hook's input and pass what it says to the
/// gerdr Cursor runs in (by its `WAYLAND_DISPLAY`; another Cursor's go
/// nowhere). It answers nothing, so it changes nothing of what the agent
/// does, and always succeeds.
pub fn run_hook() {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
    let Ok(payload) = serde_json::from_slice::<Value>(&input) else {
        return;
    };
    let Some(event) = HookEvent::from_payload(&payload, now_ms()) else {
        return;
    };
    let display = std::env::var_os("WAYLAND_DISPLAY");
    let (Ok(message), Some(path)) = (
        serde_json::to_vec(&event),
        display.as_deref().and_then(hook_socket),
    ) else {
        return;
    };
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    let _ = socket.set_nonblocking(true);
    let _ = socket.send_to(&message, path);
}

/// `existing`, the text of a hooks.json, with `command` run for each of
/// gerdr's events in place of any older gerdr's hook (one running
/// `HOOK_COMMAND` from a program called gerdr), keeping all else as it is.
/// No file starts empty. Text that isn't a hooks.json is an error.
pub fn merge_hooks(existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut root: Value = match existing.map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => serde_json::from_str(text).map_err(|err| err.to_string())?,
        None => serde_json::json!({ "version": 1, "hooks": {} }),
    };
    let object = root.as_object_mut().ok_or("not a JSON object")?;
    object.entry("version").or_insert(Value::from(1));
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or("\"hooks\" isn't an object")?;
    for event in EVENTS {
        let list = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("\"{event}\" isn't a list"))?;
        list.retain(|hook| {
            !hook
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(is_gerdr_hook)
        });
        list.push(serde_json::json!({ "command": command, "timeout": HOOK_TIMEOUT }));
    }
    serde_json::to_string_pretty(&root)
        .map(|text| text + "\n")
        .map_err(|err| err.to_string())
}

/// Whether hook `command` is a gerdr's.
fn is_gerdr_hook(command: &str) -> bool {
    let Some(program) = command.trim().strip_suffix(HOOK_COMMAND) else {
        return false;
    };
    let program = program.trim().trim_matches(|c| c == '\'' || c == '"');
    Path::new(program)
        .file_name()
        .is_some_and(|name| name == "gerdr")
}

/// Add this gerdr to Cursor's hooks, in `~/.cursor/hooks.json`, unless it
/// is there already.
pub fn install_hooks(home: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let command = format!(
        "{} {HOOK_COMMAND}",
        crate::desktop::shell_quote(&exe.to_string_lossy())
    );
    let path = home.join(".cursor/hooks.json");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let merged = merge_hooks(existing.as_deref(), &command)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(());
    }
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.gerdr-tmp");
        std::fs::write(&tmp, &merged)?;
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|err| format!("{}: {err}", path.display()))
}

impl crate::Gerdr {
    /// Follow what gerdr's Cursor's chats do: read its database off the
    /// event loop, hear its hooks, and look again every `DB_INTERVAL`, for
    /// a hook's word gets old.
    pub fn watch_cursor_agents(&mut self) {
        use smithay::reexports::calloop::{
            Interest, Mode, PostAction, channel,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        };
        let Some(db) = crate::launch::cursor_profile()
            .map(|profile| profile.join("User/globalStorage/state.vscdb"))
        else {
            return;
        };
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(rows) = event {
                state.cursor.set_rows(rows);
                state.cursor_changed();
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("cursor's agents: {err}");
        }
        self.cursor_poke = Some(watch_db(db, move |rows| tx.send(rows).is_ok()));
        match hook_socket(&self.socket_name).map(|path| (bind_hook_socket(&path), path)) {
            Some((Ok(socket), path)) => {
                let source = Generic::new(socket, Interest::READ, Mode::Level);
                let inserted = self.loop_handle.insert_source(source, |_, socket, state| {
                    for event in take_events(socket) {
                        state.cursor.hear(event);
                    }
                    state.cursor_changed();
                    if let Some(poke) = &state.cursor_poke {
                        let _ = poke.send(());
                    }
                    Ok(PostAction::Continue)
                });
                match inserted {
                    Ok(_) => self.cursor_socket = Some(path),
                    Err(err) => tracing::warn!("cursor's hooks: {err}"),
                }
            }
            Some((Err(err), path)) => tracing::warn!("cursor's hooks, {}: {err}", path.display()),
            None => tracing::warn!("cursor's hooks: no XDG_RUNTIME_DIR"),
        }
        let timer = Timer::from_duration(DB_INTERVAL);
        let tick = |_, _: &mut (), state: &mut crate::Gerdr| {
            if !state.cursor.hooks.is_empty() {
                state.cursor_changed();
            }
            TimeoutAction::ToDuration(DB_INTERVAL)
        };
        if let Err(err) = self.loop_handle.insert_source(timer, tick) {
            tracing::warn!("cursor's agents: {err}");
        }
    }

    /// Take in what Cursor's chats do now, and notify of what is news.
    fn cursor_changed(&mut self) {
        self.cursor_chats = self.cursor.live(now_ms());
        self.notify_agents();
    }

    /// Stop hearing Cursor's hooks: gerdr quits.
    pub fn close_cursor_socket(&mut self) {
        if let Some(path) = self.cursor_socket.take() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// The workspace of the shown project Cursor chat `chat` goes under
    /// (see `place`).
    pub fn cursor_chat_place(&self, chat: &CursorChat) -> Option<usize> {
        let spots: Vec<Spot> = self
            .workspaces
            .iter()
            .map(|ws| Spot {
                dir: &ws.dir,
                cursor: ws.kind == crate::WorkspaceKind::Cursor,
            })
            .collect();
        let records = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id));
        let mut dirs: Vec<&Path> = records
            .into_iter()
            .flat_map(|p| p.workspaces.iter().map(|r| r.dir.as_path()))
            .collect();
        dirs.extend(self.workspaces.iter().map(|ws| ws.dir.as_path()));
        place(&chat.row.paths, &spots, &dirs, crate::home_dir().as_deref())
    }

    /// The cards of the Cursor chats under workspace `w`.
    pub fn cursor_cards(&self, w: usize) -> Vec<crate::chrome::AgentView> {
        self.cursor_chats
            .iter()
            .filter(|chat| self.cursor_chat_place(chat) == Some(w))
            .map(crate::chrome::AgentView::cursor)
            .collect()
    }

    /// Cursor's chats as notifications see them: seen while gerdr's
    /// window has the keyboard and shows the project's Cursor.
    pub fn cursor_watched(&self) -> Vec<crate::agents::Watched> {
        let shown = self
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(w, ws)| {
                ws.tabs
                    .iter()
                    .position(|t| t.panes.iter().any(|p| is_cursor(p.app_command.as_deref())))
                    .map(|t| (w, t))
            })
            .is_some_and(|(w, t)| self.is_visible(w, t));
        self.cursor_chats
            .iter()
            .filter_map(|chat| {
                let w = self.cursor_chat_place(chat)?;
                let card = crate::chrome::AgentView::cursor(chat);
                let mut body = vec![self.workspaces[w].name.as_str()];
                body.extend(card.title());
                Some(crate::agents::Watched {
                    key: card.key.clone(),
                    status: chat.status,
                    label: card.label(),
                    body: body.join(" · "),
                    seen: self.host_focused && shown,
                })
            })
            .collect()
    }
}

/// Whether app `command` starts Cursor.
fn is_cursor(command: Option<&str>) -> bool {
    command
        .and_then(crate::launch::program_name)
        .is_some_and(|name| name == "cursor")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str) -> ChatRow {
        ChatRow {
            id: id.into(),
            updated_ms: 1_000,
            ..ChatRow::default()
        }
    }

    fn hook(event: &str, status: Option<&str>, at_ms: i64) -> HookEvent {
        HookEvent {
            event: event.into(),
            chat: "c1".into(),
            status: status.map(Into::into),
            model: None,
            roots: Vec::new(),
            prompt: None,
            at_ms,
        }
    }

    #[test]
    fn a_row_says_what_its_chat_does() {
        let mut r = row("c1");
        assert_eq!(r.status(), AgentStatus::Idle);
        r.unread = true;
        assert_eq!(r.status(), AgentStatus::Done);
        r.status = Some("generating".into());
        assert_eq!(r.status(), AgentStatus::Working);
        r.blocking = true;
        assert_eq!(r.status(), AgentStatus::Blocked);
        // An old run left unfinished doesn't make it run.
        let mut stale = row("c2");
        stale.status = Some("aborted".into());
        assert_eq!(stale.status(), AgentStatus::Idle);
    }

    #[test]
    fn a_fresh_hook_holds_over_an_older_row() {
        let mut r = row("c1");
        let start = hook("beforeSubmitPrompt", None, 2_000);
        assert_eq!(merged_status(&r, Some(&start), 3_000), AgentStatus::Working);
        let done = hook("stop", Some("completed"), 2_000);
        assert_eq!(merged_status(&r, Some(&done), 3_000), AgentStatus::Done);
        let aborted = hook("stop", Some("aborted"), 2_000);
        assert_eq!(merged_status(&r, Some(&aborted), 3_000), AgentStatus::Idle);
        // Too old: the row knows better; a run holds longer.
        let late = 2_000 + HOOK_HOLD_MS + 1;
        assert_eq!(merged_status(&r, Some(&done), late), AgentStatus::Idle);
        assert_eq!(merged_status(&r, Some(&start), late), AgentStatus::Working);
        let later = 2_000 + WORKING_HOLD_MS + 1;
        assert_eq!(merged_status(&r, Some(&start), later), AgentStatus::Idle);
        // A newer row knows better.
        r.updated_ms = 2_500;
        assert_eq!(merged_status(&r, Some(&start), 3_000), AgentStatus::Idle);
        // Only the row says it waits.
        r.updated_ms = 1_000;
        r.blocking = true;
        assert_eq!(merged_status(&r, Some(&done), 3_000), AgentStatus::Blocked);
    }

    #[test]
    fn only_chats_doing_something_are_live() {
        let mut chats = Chats::default();
        let mut news = row("news");
        news.unread = true;
        news.updated_ms = 5_000;
        let mut runs = row("runs");
        runs.status = Some("generating".into());
        chats.set_rows(vec![row("idle"), news, runs]);
        let live: Vec<_> = chats.live(6_000).into_iter().map(|c| c.row.id).collect();
        assert_eq!(live, ["news", "runs"]);
        // A hook wakes an idle one; its session's end forgets it.
        let mut start = hook("preToolUse", None, 5_500);
        start.chat = "idle".into();
        chats.hear(start.clone());
        assert_eq!(chats.live(6_000).len(), 3);
        start.event = "sessionEnd".into();
        chats.hear(start);
        assert_eq!(chats.live(6_000).len(), 2);
        // One only hooks know of shows by its prompt, in its folders.
        let mut first = hook("beforeSubmitPrompt", None, 5_500);
        first.chat = "new".into();
        first.roots = vec![PathBuf::from("/src/app")];
        first.prompt = Some("run the tests".into());
        first.model = Some("grok".into());
        chats.hear(first);
        let mut tool = hook("preToolUse", None, 5_600);
        tool.chat = "new".into();
        chats.hear(tool);
        let new = chats
            .live(6_000)
            .into_iter()
            .find(|c| c.row.id == "new")
            .unwrap();
        assert_eq!(new.status, AgentStatus::Working);
        assert_eq!(new.row.name.as_deref(), Some("run the tests"));
        assert_eq!(new.row.paths, [PathBuf::from("/src/app")]);
        assert_eq!(new.model.as_deref(), Some("grok"));
        // Once the database has it, its row names it, in the hook's folders
        // if the row has none.
        let mut row_new = row("new");
        row_new.name = Some("Tests".into());
        chats.rows.insert("new".into(), row_new);
        let new = chats
            .live(6_000)
            .into_iter()
            .find(|c| c.row.id == "new")
            .unwrap();
        assert_eq!(new.row.name.as_deref(), Some("Tests"));
        assert_eq!(new.row.paths, [PathBuf::from("/src/app")]);
        // What hooks said long ago goes.
        chats.live(5_600 + WORKING_HOLD_MS + 1);
        assert!(chats.hooks.is_empty());
    }

    #[test]
    fn hook_input_is_read() {
        let payload = serde_json::json!({
            "hook_event_name": "stop",
            "conversation_id": "c1",
            "status": "completed",
            "model": "gpt-5",
            "workspace_roots": ["/src/gerdr"],
            "loop_count": 0,
        });
        let event = HookEvent::from_payload(&payload, 7).unwrap();
        assert_eq!(
            event,
            HookEvent {
                event: "stop".into(),
                chat: "c1".into(),
                status: Some("completed".into()),
                model: Some("gpt-5".into()),
                roots: vec![PathBuf::from("/src/gerdr")],
                prompt: None,
                at_ms: 7,
            }
        );
        let long = "x".repeat(PROMPT_CHARS + 5);
        let asked = serde_json::json!({
            "hook_event_name": "beforeSubmitPrompt",
            "conversation_id": "c1",
            "prompt": format!("\n  {long}\nmore"),
        });
        let prompt = HookEvent::from_payload(&asked, 7).unwrap().prompt.unwrap();
        assert_eq!(prompt, format!("{}…", &long[..PROMPT_CHARS]));
        let bare = serde_json::json!({ "hook_event_name": "stop", "session_id": "c2" });
        assert_eq!(HookEvent::from_payload(&bare, 7).unwrap().chat, "c2");
        assert!(
            HookEvent::from_payload(&serde_json::json!({ "conversation_id": "c" }), 7).is_none()
        );
        // It goes over the socket and back.
        let text = serde_json::to_vec(&event).unwrap();
        assert_eq!(serde_json::from_slice::<HookEvent>(&text).unwrap(), event);
    }

    #[test]
    fn chats_go_under_the_workspace_holding_their_repo() {
        let home = Path::new("/home/u");
        let spots = [
            Spot {
                dir: home,
                cursor: false,
            },
            Spot {
                dir: Path::new("/home/u/app"),
                cursor: false,
            },
            Spot {
                dir: Path::new("/home/u/app/api"),
                cursor: false,
            },
            Spot {
                dir: Path::new("/home/u/app"),
                cursor: true,
            },
        ];
        let project = [Path::new("/home/u/app")];
        let at = |paths: &[&str]| {
            let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            place(&paths, &spots, &project, Some(home))
        };
        assert_eq!(at(&["/home/u/app/api/src"]), Some(2));
        assert_eq!(at(&["/home/u/app/web"]), Some(1));
        // A worktree elsewhere goes by the repo it came from.
        assert_eq!(
            at(&["/home/u/.cursor/worktrees/api/x", "/home/u/app/api"]),
            Some(2)
        );
        // Not the home folder's workspace: another project's chat.
        assert_eq!(at(&["/home/u/other"]), None);
        // In the project but under no workspace: the Cursor one.
        let lone = [Spot {
            dir: Path::new("/home/u/app"),
            cursor: true,
        }];
        let paths = [PathBuf::from("/home/u/app/web")];
        assert_eq!(place(&paths, &lone, &project, Some(home)), Some(0));
    }

    #[test]
    fn gerdrs_hooks_join_the_users() {
        let command = "/opt/gerdr/gerdr cursor-hook";
        let fresh: Value = serde_json::from_str(&merge_hooks(None, command).unwrap()).unwrap();
        assert_eq!(fresh["version"], 1);
        assert_eq!(fresh["hooks"]["stop"][0]["command"], command);
        let theirs = r#"{"version":1,"hooks":{"stop":[{"command":"notify-send done"},
            {"command":"'/old/gerdr' cursor-hook"}],"afterFileEdit":[{"command":"fmt"}]},"x":2}"#;
        let merged = merge_hooks(Some(theirs), command).unwrap();
        let value: Value = serde_json::from_str(&merged).unwrap();
        let stop = value["hooks"]["stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["command"], "notify-send done");
        assert_eq!(stop[1]["command"], command);
        assert_eq!(value["hooks"]["afterFileEdit"][0]["command"], "fmt");
        assert_eq!(value["x"], 2);
        // Merging again changes nothing.
        assert_eq!(merge_hooks(Some(&merged), command).unwrap(), merged);
        assert!(merge_hooks(Some("{oops"), command).is_err());
        assert!(merge_hooks(Some("[]"), command).is_err());
    }

    #[test]
    fn every_gerdrs_socket_is_found_by_its_name() {
        let name = |socket: &str| {
            let path = hook_socket(std::ffi::OsStr::new(socket))?;
            Some(path.file_name()?.to_string_lossy().into_owned())
        };
        if runtime_dir().is_some() {
            assert_eq!(name("gerdr-0").as_deref(), Some("gerdr-0.cursor"));
            assert_eq!(name("wayland-3").as_deref(), Some("gerdr-wayland-3.cursor"));
        }
    }

    #[test]
    fn hooks_reach_a_listening_gerdr() {
        let dir = std::env::temp_dir().join(format!("gerdr-cursorhook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gerdr-test.cursor");
        let socket = bind_hook_socket(&path).unwrap();
        let event = hook("stop", Some("completed"), 1);
        let sender = UnixDatagram::unbound().unwrap();
        sender
            .send_to(&serde_json::to_vec(&event).unwrap(), &path)
            .unwrap();
        assert_eq!(take_events(&socket), [event]);
        assert!(take_events(&socket).is_empty());
        // One no gerdr listens on goes when the next binds.
        let dead = dir.join("gerdr-dead.cursor");
        drop(bind_hook_socket(&dead).unwrap());
        assert!(dead.exists());
        let again = bind_hook_socket(&path).unwrap();
        assert!(!dead.exists() && path.exists());
        sender.send_to(&[], &path).unwrap();
        assert!(take_events(&again).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_are_read_from_cursors_tables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);
             CREATE TABLE composerHeaders (composerId TEXT PRIMARY KEY, workspaceId TEXT,
                createdAt INTEGER, lastUpdatedAt INTEGER, isArchived INTEGER,
                isSubagent INTEGER, recency INTEGER, checkpointAt INTEGER, value TEXT);",
        )
        .unwrap();
        let now = 100 * RECENT_MS;
        let header = |id: &str, at: i64, archived: i64, value: &str| {
            conn.execute(
                "INSERT INTO composerHeaders (composerId, lastUpdatedAt, isArchived, isSubagent, value)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                rusqlite::params![id, at, archived, value],
            )
            .unwrap();
        };
        header(
            "a",
            now - 10,
            0,
            r#"{"name":"Fix it","hasBlockingPendingActions":true,"hasUnreadMessages":false,
               "agentLocation":{"type":"worktree","environment":{"uri":{"fsPath":"/wt/a"}},
               "sourceRepoRootPath":"/src/app"},"trackedGitRepos":[{"repoPath":"/src/lib"}]}"#,
        );
        header("old", now - 2 * RECENT_MS, 0, r#"{"name":"Old"}"#);
        header(
            "unread",
            now - 2 * RECENT_MS,
            0,
            r#"{"hasUnreadMessages":true}"#,
        );
        header("gone", now - 10, 1, r#"{"name":"Archived"}"#);
        conn.execute(
            "INSERT INTO cursorDiskKV VALUES ('composerData:a', '{\"status\":\"generating\"}')",
            [],
        )
        .unwrap();
        let mut known = HashMap::new();
        let mut rows = read_rows(&conn, now, &mut known).unwrap();
        rows.sort_by(|x, y| x.id.cmp(&y.id));
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "unread"]);
        let a = &rows[0];
        assert_eq!(a.name.as_deref(), Some("Fix it"));
        assert!(a.blocking && !a.unread && a.worktree);
        assert_eq!(a.status.as_deref(), Some("generating"));
        let paths: Vec<&str> = a.paths.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(paths, ["/wt/a", "/src/app", "/src/lib"]);
        assert!(rows[1].unread);
        assert_eq!(known.len(), 2);
    }
}
