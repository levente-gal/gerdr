//! Where each agent CLI keeps a chat: Claude's `projects/*/<id>.jsonl`,
//! Cursor's `chats/*/<id>/store.db`, Codex's `sessions/*/*/*/rollout-*.jsonl`,
//! OpenCode's database and T3 Code's. On another machine they are looked
//! up by a script and pulled into `.staging` first.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

use super::{AgentKind, Chat};
use crate::integrations::common::{
    files::first_line,
    pick::{Begun, nearest},
};

/// A Codex chat found by time began this long before its run at most.
const CODEX_BEFORE: u64 = 60;
/// A Codex chat found by time began this long after its run at most.
const CODEX_AFTER: u64 = 600;
/// The most of a rollout's first line read: its `session_meta`.
pub(crate) const FIRST_LINE: u64 = 1 << 20;

/// When a chat found by folder and time began: from `from` to `to`, the
/// one begun nearest `near` picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: u64,
    pub to: u64,
    pub near: u64,
}

impl Span {
    /// A run's begun at `started`: from a minute before it to ten after,
    /// nearest it.
    pub fn of(started: u64) -> Self {
        Self {
            from: started.saturating_sub(CODEX_BEFORE),
            to: started + CODEX_AFTER,
            near: started,
        }
    }
}

/// A chat found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub agent: AgentKind,
    /// Its id: the one asked for, or the one found by folder and time.
    pub id: String,
    /// Its transcript, or its database.
    pub main: PathBuf,
    /// The files that go with it, by their path in `extra/`.
    pub extra: Vec<(String, PathBuf)>,
    /// What `main` is, and how it is kept.
    pub form: Form,
    /// Pieces read out of a database, by their path in `extra/`: T3 Code's
    /// thread and runs, OpenCode's session.
    pub rows: Vec<(String, Vec<u8>)>,
}

/// What a found chat's `main` is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Form {
    /// Lines that only grow (Claude, Codex): kept as frames appended.
    Lines,
    /// A Cursor `store.db`: kept as a snapshot, taken again when it moved.
    Store,
    /// The chat as lines read out of `main`, a database (OpenCode).
    Made(Vec<u8>),
    /// No transcript of its own: only `rows` (a T3 thread whose agent's
    /// chat wasn't found).
    Nothing,
}

/// Where the agent CLIs keep their chats: the home's, or a test's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Homes {
    /// Claude Code's config folders: `$CLAUDE_CONFIG_DIR`, then `~/.claude`.
    pub claude: Vec<PathBuf>,
    /// `~/.cursor`.
    pub cursor: PathBuf,
    /// `$CODEX_HOME`, else `~/.codex`.
    pub codex: PathBuf,
    /// OpenCode's database.
    pub opencode: PathBuf,
    /// T3 Code's home, when it ran here.
    pub t3: Option<PathBuf>,
    /// Where pi keeps its sessions: `~/.pi/agent/sessions`.
    pub pi: PathBuf,
    /// The Antigravity CLI's home: `~/.gemini/antigravity-cli`.
    pub antigravity: PathBuf,
}

impl Homes {
    /// This machine's. None without a home.
    pub fn here() -> Option<Self> {
        let home = crate::home_dir()?;
        let env = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
        };
        let mut homes = Self::under(&home);
        if let Some(config) = env("CLAUDE_CONFIG_DIR")
            && !homes.claude.contains(&config)
        {
            homes.claude.insert(0, config);
        }
        if let Some(codex) = env("CODEX_HOME") {
            homes.codex = codex;
        }
        if let Some(data) = env("XDG_DATA_HOME") {
            homes.opencode = data.join("opencode").join("opencode.db");
        }
        if let Some(sessions) = env("PI_CODING_AGENT_SESSION_DIR") {
            homes.pi = sessions;
        } else if let Some(agent) = env("PI_CODING_AGENT_DIR") {
            homes.pi = agent.join("sessions");
        }
        homes.t3 = crate::t3_agents::home();
        Some(homes)
    }

    /// All of them under home folder `home`, as each CLI keeps them by
    /// default; no T3 Code.
    pub fn under(home: &Path) -> Self {
        Self {
            claude: vec![home.join(".claude")],
            cursor: home.join(".cursor"),
            codex: home.join(".codex"),
            opencode: home.join(".local/share/opencode/opencode.db"),
            t3: None,
            pi: home.join(".pi/agent/sessions"),
            antigravity: home.join(".gemini/antigravity-cli"),
        }
    }
}

/// `chat` among `homes`, if it is there; ids in `claimed` belong to other
/// runs. An error: a database that couldn't be read.
pub fn find(
    homes: &Homes,
    chat: &Chat,
    claimed: &HashSet<String>,
) -> Result<Option<Found>, String> {
    find_in(homes, chat, claimed, Span::of(chat.started))
}

/// `chat` as `find` finds it, one without an id begun in `span`.
pub fn find_in(
    homes: &Homes,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Result<Option<Found>, String> {
    let id = chat.id.as_deref().filter(|id| plain_id(id));
    if chat.id.is_some() && id.is_none() {
        return Err(format!("{:?} isn't a chat id", chat.id));
    }
    crate::integrations::of(chat.agent).find(homes, chat, id, claimed, span)
}

/// Whether `id` can go into a path and a script as it is.
pub fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && (id.chars()).all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The files under `base`'s folders `subs`, by their path from `base`, in
/// order. Links aren't followed.
fn files_under(base: &Path, subs: &[&str]) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
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
    for sub in subs {
        walk(&base.join(sub), &mut paths);
    }
    let mut files: Vec<(String, PathBuf)> = (paths.into_iter())
        .filter_map(|path| {
            let rel = path.strip_prefix(base).ok()?.to_str()?.to_owned();
            Some((rel, path))
        })
        .collect();
    files.sort();
    files
}

pub(crate) fn found(agent: AgentKind, id: &str, main: PathBuf, form: Form) -> Found {
    Found {
        agent,
        id: id.to_owned(),
        main,
        extra: Vec::new(),
        form,
        rows: Vec::new(),
    }
}

/// Claude Code chat `id`: `projects/<folder>/<id>.jsonl` of a config
/// folder, and its `<id>/tool-results` and `<id>/subagents`.
pub fn claude(homes: &Homes, id: &str) -> Option<Found> {
    let name = format!("{id}.jsonl");
    homes.claude.iter().find_map(|config| {
        let folders = std::fs::read_dir(config.join("projects")).ok()?;
        let main = (folders.flatten())
            .map(|dir| dir.path().join(&name))
            .find(|path| path.is_file())?;
        let mut chat = found(AgentKind::Claude, id, main.clone(), Form::Lines);
        chat.extra = files_under(&main.with_extension(""), &["tool-results", "subagents"]);
        Some(chat)
    })
}

/// The folders of `dir`, by name in order.
pub(crate) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = (entries.flatten())
        .filter(|e| e.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// pi's sessions under `sessions`, with when each began by its name
/// (`<yyyy-mm-ddThh-mm-ss-mmmZ>_<id>.jsonl`), in a folder per working
/// folder.
fn pi_sessions(sessions: &Path) -> Vec<(PathBuf, u64)> {
    (subdirs(sessions).iter())
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let (time, _) = name.strip_suffix(".jsonl")?.split_once('_')?;
            // `2026-11-24T19-27-30-349Z` as ISO: its clock's dashes as colons.
            let (date, clock) = time.split_once('T')?;
            let mut parts = clock.trim_end_matches('Z').split('-');
            let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
            let at = super::usage::iso_ms(&format!("{date}T{h}:{m}:{s}Z"))? / 1000;
            Some((path, at))
        })
        .collect()
}

/// pi chat `id`: `sessions/--<folder>--/<time>_<id>.jsonl`.
pub(crate) fn pi_by_id(sessions: &Path, id: &str) -> Option<Found> {
    let end = format!("_{id}.jsonl");
    let (main, _) = (pi_sessions(sessions).into_iter())
        .find(|(path, _)| path.to_str().is_some_and(|p| p.ends_with(&end)))?;
    Some(found(AgentKind::Pi, id, main, Form::Lines))
}

/// The pi chat of `chat`'s run, by folder and time: its first line says
/// where it runs (`{"type":"session","id","timestamp","cwd"}`).
pub(crate) fn pi_by_time(
    sessions: &Path,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Option<Found> {
    let metas: Vec<(PathBuf, Begun)> = (pi_sessions(sessions).into_iter())
        .filter(|(_, at)| (span.from..=span.to).contains(at))
        .filter_map(|(path, _)| {
            let head: Value =
                serde_json::from_str(first_line(&path, FIRST_LINE, false)?.trim()).ok()?;
            if head["type"] != "session" {
                return None;
            }
            let meta = Begun {
                id: head["id"].as_str()?.to_owned(),
                cwd: PathBuf::from(head["cwd"].as_str()?),
                started: super::usage::iso_ms(head["timestamp"].as_str()?)? / 1000,
            };
            Some((path, meta))
        })
        .collect();
    let at = nearest(
        chat,
        span,
        claimed,
        (metas.iter().enumerate()).map(|(i, (_, m))| (i, m)),
    )?;
    let (main, meta) = &metas[at];
    plain_id(&meta.id).then(|| found(AgentKind::Pi, &meta.id, main.clone(), Form::Lines))
}

/// The Antigravity CLI's transcript of conversation `id`, the whole one:
/// `brain/<id>/.system_generated/logs/transcript_full.jsonl`.
pub fn antigravity_transcript(home: &Path, id: &str) -> PathBuf {
    (home.join("brain").join(id)).join(".system_generated/logs/transcript_full.jsonl")
}

/// Antigravity CLI conversation `id`.
pub(crate) fn antigravity_by_id(home: &Path, id: &str) -> Option<Found> {
    let main = antigravity_transcript(home, id);
    main.is_file()
        .then(|| found(AgentKind::Antigravity, id, main, Form::Lines))
}

/// The Antigravity CLI conversation of `chat`'s run, by folder and time:
/// its `history.jsonl` has a line per prompt typed, with the conversation
/// and the folder (`{timestamp, workspace, conversationId}`); a
/// conversation begins with its first.
pub(crate) fn antigravity_by_time(
    home: &Path,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Option<Found> {
    let history = std::fs::read_to_string(home.join("history.jsonl")).ok()?;
    let mut metas: Vec<Begun> = Vec::new();
    for line in history.lines() {
        let Ok(line) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(id), Some(cwd), Some(ms)) = (
            line["conversationId"].as_str(),
            line["workspace"].as_str(),
            line["timestamp"].as_u64(),
        ) else {
            continue;
        };
        if !metas.iter().any(|m| m.id == id) {
            let (id, cwd, started) = (id.to_owned(), PathBuf::from(cwd), ms / 1000);
            metas.push(Begun { id, cwd, started });
        }
    }
    let meta = nearest(chat, span, claimed, metas.iter().map(|m| (m, m)))?;
    plain_id(&meta.id)
        .then(|| antigravity_by_id(home, &meta.id))
        .flatten()
}

/// `value` of a database row as JSON: texts of JSON columns (`*_json`,
/// `data`, `metadata`) as what they hold.
fn cell(name: &str, value: rusqlite::types::ValueRef) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(x) => Value::from(x),
        ValueRef::Text(text) => {
            let text = String::from_utf8_lossy(text);
            let json = name.ends_with("_json") || matches!(name, "data" | "metadata");
            (json.then(|| serde_json::from_str(&text).ok()).flatten())
                .unwrap_or_else(|| Value::String(text.into_owned()))
        }
        ValueRef::Blob(bytes) => Value::from(format!("<{} bytes>", bytes.len())),
    }
}

/// The rows `sql` gives for `arg`, each as a JSON object by column.
fn rows_json(conn: &rusqlite::Connection, sql: &str, arg: &str) -> rusqlite::Result<Vec<Value>> {
    rows_of(conn, sql, [arg])
}

/// The rows `sql` gives for `params`, each as a JSON object by column.
pub(crate) fn rows_of(
    conn: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> rusqlite::Result<Vec<Value>> {
    let mut query = conn.prepare(sql)?;
    let names: Vec<String> = query.column_names().iter().map(|n| n.to_string()).collect();
    let rows = query.query_map(params, |row| {
        let mut object = Map::new();
        for (i, name) in names.iter().enumerate() {
            object.insert(name.clone(), cell(name, row.get_ref(i)?));
        }
        Ok(Value::Object(object))
    })?;
    rows.collect()
}

/// The columns of table `table`; none when it isn't there.
fn columns(conn: &rusqlite::Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut query = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let names = query.query_map([table], |row| row.get(0))?;
    names.collect()
}

fn pretty(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).unwrap_or_default()
}

/// The OpenCode chat of `chat`: by its id, or by folder and time. Its
/// schema isn't known for sure, so it is asked first; a database not as
/// expected finds nothing.
pub(crate) fn opencode(
    db: &Path,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Result<Option<Found>, String> {
    if !db.is_file() {
        return Ok(None);
    }
    let read = || -> rusqlite::Result<Option<Found>> {
        let conn = crate::cursor_agents::open_read_only(db)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let has = |table: &str, want: &[&str]| -> rusqlite::Result<bool> {
            let cols = columns(&conn, table)?;
            Ok(want.iter().all(|w| cols.iter().any(|c| c == w)))
        };
        let need = ["id", "directory", "time_created"];
        // OpenCode 2 keeps both: older chats in the first tables, newer
        // ones only in the second.
        let mut schemas = Vec::new();
        if has("session", &need)? && has("message", &["session_id", "data"])? {
            schemas.push(("session", false));
        }
        if has("session_v2", &need)? && has("session_message", &["session_id", "seq", "data"])? {
            schemas.push(("session_v2", true));
        }
        let mut hit = None;
        for (session, v2) in schemas {
            if let Some(row) = opencode_session(&conn, session, chat, claimed, span)? {
                hit = Some((row, v2));
                break;
            }
        }
        let Some((session_row, v2)) = hit else {
            return Ok(None);
        };
        let id = session_row["id"].as_str().unwrap_or_default().to_owned();
        if !plain_id(&id) {
            return Ok(None);
        }
        let mut lines = Vec::new();
        let messages = match v2 {
            false => rows_json(
                &conn,
                "SELECT * FROM message WHERE session_id = ?1 ORDER BY time_created, id",
                &id,
            )?,
            true => rows_json(
                &conn,
                "SELECT * FROM session_message WHERE session_id = ?1 ORDER BY seq",
                &id,
            )?,
        };
        let parts = match !v2 && has("part", &["message_id", "session_id", "data"])? {
            true => rows_json(
                &conn,
                "SELECT * FROM part WHERE session_id = ?1 ORDER BY time_created, id",
                &id,
            )?,
            false => Vec::new(),
        };
        for mut message in messages {
            let own: Vec<Value> = (parts.iter())
                .filter(|p| p["message_id"] == message["id"])
                .cloned()
                .collect();
            if !own.is_empty() {
                message["parts"] = Value::Array(own);
            }
            lines.extend(serde_json::to_vec(&message).unwrap_or_default());
            lines.push(b'\n');
        }
        let mut chat = found(
            AgentKind::OpenCode,
            &id,
            db.to_path_buf(),
            Form::Made(lines),
        );
        chat.rows = vec![("opencode/session.json".to_owned(), pretty(&session_row))];
        Ok(Some(chat))
    };
    read().map_err(|err| format!("reading {}: {err}", db.display()))
}

/// The session row of `chat` in OpenCode's table `table`.
fn opencode_session(
    conn: &rusqlite::Connection,
    table: &str,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> rusqlite::Result<Option<Value>> {
    if let Some(id) = &chat.id {
        let rows = rows_json(conn, &format!("SELECT * FROM {table} WHERE id = ?1"), id)?;
        return Ok(rows.into_iter().next());
    }
    // OpenCode's times are in milliseconds.
    let (from, to) = (span.from * 1000, span.to * 1000);
    let rows = rows_json(
        conn,
        &format!(
            "SELECT * FROM {table} WHERE time_created BETWEEN {from} AND {to} AND ?1 = ?1 \
             ORDER BY time_created"
        ),
        "",
    )?;
    let near = span.near * 1000;
    Ok((rows.into_iter())
        .filter(|row| row["parent_id"].is_null())
        .filter(|row| !row["id"].as_str().is_some_and(|id| claimed.contains(id)))
        .filter(|row| {
            let dir = Path::new(row["directory"].as_str().unwrap_or(""));
            !dir.as_os_str().is_empty() && chat.folders.iter().any(|f| dir.starts_with(f))
        })
        .min_by_key(|row| row["time_created"].as_u64().unwrap_or(0).abs_diff(near)))
}

/// A T3 Code thread's rows: the thread, its runs, its agents' threads.
type T3Rows = (Vec<Value>, Vec<Value>, Vec<Value>);

/// T3 Code thread `id`: its row, runs and agent threads, then the chat of
/// the agent it ran (Claude, Codex or Cursor) when that is found.
pub(crate) fn t3(homes: &Homes, chat: &Chat, id: &str) -> Result<Option<Found>, String> {
    let Some(home) = &homes.t3 else {
        return Ok(None);
    };
    let db = home.join(crate::t3_agents::DB);
    if !db.is_file() {
        return Ok(None);
    }
    let read = || -> rusqlite::Result<Option<T3Rows>> {
        let conn = crate::t3_agents::open_read_only(&db)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let threads = rows_json(
            &conn,
            "SELECT * FROM orchestration_v2_projection_threads WHERE thread_id = ?1",
            id,
        )?;
        if threads.is_empty() {
            return Ok(None);
        }
        let runs = rows_json(
            &conn,
            "SELECT * FROM orchestration_v2_projection_runs WHERE thread_id = ?1 ORDER BY ordinal",
            id,
        )?;
        // Newer than the schema gerdr reads threads by: missing is no error.
        let agents = rows_json(
            &conn,
            "SELECT * FROM orchestration_v2_projection_provider_threads WHERE thread_id = ?1 \
             ORDER BY updated_at",
            id,
        )
        .unwrap_or_default();
        Ok(Some((threads, runs, agents)))
    };
    let Some((threads, runs, agents)) =
        read().map_err(|err| format!("reading {}: {err}", db.display()))?
    else {
        return Ok(None);
    };
    let rows = vec![
        ("t3/thread.json".to_owned(), pretty(&Value::Array(threads))),
        ("t3/runs.json".to_owned(), pretty(&Value::Array(runs))),
        (
            "t3/agent-threads.json".to_owned(),
            pretty(&Value::Array(agents.clone())),
        ),
    ];
    let native = agents.iter().rev().find_map(|row| {
        let native = &row["payload_json"]["nativeThreadRef"];
        let driver = native["driver"].as_str().or(row["provider"].as_str())?;
        let native_id = native["nativeId"].as_str()?;
        let agent = match AgentKind::of(driver) {
            AgentKind::Claude => (AgentKind::Claude, native_id),
            AgentKind::Codex => (AgentKind::Codex, native_id),
            AgentKind::Cursor => (AgentKind::Cursor, native_id.trim_start_matches("agent-")),
            _ => return None,
        };
        plain_id(agent.1).then(|| (agent.0, agent.1.to_owned()))
    });
    let theirs = native.and_then(|(agent, native_id)| {
        let inner = Chat {
            agent,
            id: Some(native_id),
            ..chat.clone()
        };
        // The thread's rows are kept all the same.
        (find(homes, &inner, &HashSet::new()))
            .inspect_err(|err| tracing::warn!("finding the chat of T3 thread {id}: {err}"))
            .ok()
            .flatten()
    });
    let mut kept = theirs.unwrap_or_else(|| found(AgentKind::T3, id, db.clone(), Form::Nothing));
    kept.rows.extend(rows);
    Ok(Some(kept))
}

/// One file a script found there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub path: String,
    pub len: u64,
    /// In seconds since the epoch.
    pub mtime: u64,
}

/// What a script found of a chat on another machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    pub agent: AgentKind,
    pub id: String,
    /// Its transcript, or its `store.db`.
    pub main: Remote,
    /// A Cursor `store.db`'s `-wal`.
    pub wal: Option<Remote>,
    /// The files that go with it, by their path in `extra/`.
    pub extra: Vec<(String, Remote)>,
}

/// The shell lines that print a file as `script`'s lines go: `say <tag>
/// <rel> <path>` prints its tag, "size mtime", rel and path, apart by tabs.
const SAY: &str = r#"st() { stat -c '%s %Y' "$1" 2>/dev/null || stat -f '%z %m' "$1"; }
say() { printf '%s\t%s\t%s\t%s\n' "$1" "$(st "$3")" "$2" "$3"; }
"#;

/// The script that lists `chat`'s files on another machine, for `here`'s
/// search there. Its lines are read by `read_listing`. None for what isn't
/// kept from there: OpenCode and T3 Code chats, and ids that aren't plain.
pub fn script(chat: &Chat) -> Option<String> {
    let id = chat.id.as_deref();
    if id.is_some_and(|id| !plain_id(id)) {
        return None;
    }
    let body = crate::integrations::of(chat.agent).remote_script(chat, id)?;
    Some(format!("{SAY}{body}exit 0\n"))
}

/// The body of `script` for Claude Code chat `id`.
pub(crate) fn claude_script(id: &str) -> String {
    format!(
        r#"id='{id}'
for d in "${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}" "$HOME/.claude"; do
  for f in "$d"/projects/*/"$id.jsonl"; do
    [ -f "$f" ] || continue
    say M '' "$f"
    b="${{f%.jsonl}}"
    for s in tool-results subagents; do
      [ -d "$b/$s" ] || continue
      find "$b/$s" -type f | while IFS= read -r x; do say X "${{x#"$b/"}}" "$x"; done
    done
    exit 0
  done
done
"#
    )
}

/// Whether `rel` stays inside the folder it is a path in.
fn inside(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// What `script(chat)` printed, `out`, as the chat's files there; ids in
/// `claimed` belong to other runs. None: nothing was found.
pub fn read_listing(chat: &Chat, out: &str, claimed: &HashSet<String>) -> Option<Listing> {
    let mut main = None;
    let mut wal = None;
    let mut extra = Vec::new();
    let mut candidates: Vec<(Remote, Begun)> = Vec::new();
    let mut last: Option<Remote> = None;
    for line in out.lines() {
        if let Some(first) = line.strip_prefix("L\t") {
            if let (Some(remote), Some(meta)) = (
                last.take(),
                crate::integrations::codex::find::codex_meta(first),
            ) {
                candidates.push((remote, meta));
            }
            continue;
        }
        let mut fields = line.splitn(4, '\t');
        let (Some(tag), Some(stat), Some(rel), Some(path)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let mut stat = stat.split_whitespace().map(|n| n.parse().unwrap_or(0));
        let remote = Remote {
            path: path.to_owned(),
            len: stat.next().unwrap_or(0),
            mtime: stat.next().unwrap_or(0),
        };
        match tag {
            "M" => main = main.or(Some(remote)),
            "W" => wal = Some(remote),
            "X" if inside(rel) => extra.push((rel.to_owned(), remote)),
            "C" => last = Some(remote),
            _ => {}
        }
    }
    let (main, id) = match (main, &chat.id) {
        (Some(main), Some(id)) => (main, id.clone()),
        (None, None) if crate::integrations::of(chat.agent).remote_guesses() => {
            let at = nearest(
                chat,
                Span::of(chat.started),
                claimed,
                candidates.iter().enumerate().map(|(i, (_, m))| (i, m)),
            )?;
            let (remote, meta) = candidates.swap_remove(at);
            (remote, meta.id)
        }
        _ => return None,
    };
    Some(Listing {
        agent: chat.agent,
        id,
        main,
        wal,
        extra,
    })
}

/// Where a pulled chat's files go in its staging folder: the transcript or
/// `store.db` (its `-wal` beside it), and `extra/<rel>`.
pub fn staged_main(agent: AgentKind, staging: &Path) -> PathBuf {
    match crate::integrations::of(agent).staged_store() {
        true => staging.join("store.db"),
        false => staging.join("transcript.jsonl"),
    }
}

/// The chat `id` of `agent` pulled into `staging`.
pub fn staged(agent: AgentKind, id: &str, staging: &Path) -> Option<Found> {
    let main = staged_main(agent, staging);
    if !main.is_file() {
        return None;
    }
    let form = match crate::integrations::of(agent).staged_store() {
        true => Form::Store,
        false => Form::Lines,
    };
    let mut chat = found(agent, id, main, form);
    chat.extra = files_under(&staging.join("extra"), &[""])
        .into_iter()
        .map(|(rel, path)| (rel.trim_start_matches('/').to_owned(), path))
        .collect();
    Some(chat)
}

/// Days since the epoch as a year, month and day (Howard Hinnant's
/// `civil_from_days`).
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A year, month and day as days since the epoch (`days_from_civil`).
fn days_of(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An ISO 8601 time like `2026-08-26T10:58:45.531Z` (or with `+02:00`) in
/// seconds since the epoch.
pub(crate) fn iso_secs(text: &str) -> Option<u64> {
    let num = |s: Option<&str>| s?.parse::<i64>().ok();
    let (date, time) = text.split_once('T')?;
    let mut ymd = date.splitn(3, '-');
    let (y, m, d) = (num(ymd.next())?, num(ymd.next())?, num(ymd.next())?);
    let clock_end = time.find(['Z', '+', '-']).unwrap_or(time.len());
    let (clock, zone) = time.split_at(clock_end);
    let mut hms = clock.splitn(3, ':');
    let (h, mi) = (num(hms.next())?, num(hms.next())?);
    let s = hms
        .next()
        .map_or(Some(0), |s| s.split('.').next()?.parse::<i64>().ok())?;
    let offset = match zone.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let mut parts = zone[1..].splitn(2, ':');
            let (zh, zm) = (num(parts.next())?, num(parts.next()).unwrap_or(0));
            let secs = zh * 3600 + zm * 60;
            if *sign == b'+' { secs } else { -secs }
        }
        _ => 0,
    };
    let month = u32::try_from(m).ok()?;
    let day = u32::try_from(d).ok()?;
    let secs = days_of(y, month, day) * 86_400 + h * 3600 + mi * 60 + s - offset;
    u64::try_from(secs).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        ai_work::write::tests::scratch, feature::Stage, integrations::codex::find::codex_days,
    };

    pub(crate) fn chat(
        agent: AgentKind,
        id: Option<&str>,
        started: u64,
        folders: &[&Path],
    ) -> Chat {
        Chat {
            agent,
            id: id.map(str::to_owned),
            stage: Stage::Build,
            n: 1,
            started,
            folders: folders.iter().map(|f| f.to_path_buf()).collect(),
            machine: None,
        }
    }

    pub(crate) fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn pi_and_antigravity_chats_are_found_by_id_and_by_folder_and_time() {
        let home = std::env::temp_dir().join(format!("gerdr-locate-piagy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let homes = Homes::under(&home);
        let started = 1_787_741_925; // 2026-08-26T10:58:45Z
        let (app, other) = (Path::new("/w/app"), Path::new("/w/other"));
        let session = |id: &str, cwd: &str| {
            format!(
                r#"{{"type":"session","version":3,"id":"{id}","timestamp":"2026-08-26T10:58:50.000Z","cwd":"{cwd}"}}"#
            )
        };
        put(
            &homes
                .pi
                .join("--w-app--/2026-08-26T10-58-50-000Z_p-1.jsonl"),
            &session("p-1", "/w/app"),
        );
        put(
            &homes
                .pi
                .join("--w-other--/2026-08-26T10-58-50-000Z_p-2.jsonl"),
            &session("p-2", "/w/other"),
        );
        let none = HashSet::new();
        let pi = |id: Option<&str>, folder: &Path, claimed: &HashSet<String>| {
            find(
                &homes,
                &chat(AgentKind::Pi, id, started, &[folder]),
                claimed,
            )
            .unwrap()
            .map(|f| f.id)
        };
        assert_eq!(pi(None, app, &none).as_deref(), Some("p-1"));
        assert_eq!(pi(None, other, &none).as_deref(), Some("p-2"));
        assert_eq!(
            pi(Some("p-2"), app, &none).as_deref(),
            Some("p-2"),
            "by id, any folder"
        );
        let held = HashSet::from(["p-1".to_owned()]);
        assert_eq!(pi(None, app, &held), None, "another pane's");
        // Antigravity: its history names each conversation's folder.
        let ms = (started + 5) * 1000;
        let history = format!(
            "{{\"timestamp\":{ms},\"workspace\":\"/w/other\",\"conversationId\":\"a-2\"}}\n\
             {{\"timestamp\":{ms},\"workspace\":\"/w/app\",\"conversationId\":\"a-1\"}}\n\
             {{\"timestamp\":{},\"workspace\":\"/w/app\",\"conversationId\":\"a-1\"}}\n",
            ms + 90_000
        );
        put(&homes.antigravity.join("history.jsonl"), &history);
        for id in ["a-1", "a-2"] {
            put(&antigravity_transcript(&homes.antigravity, id), "{}\n");
        }
        let agy = |id: Option<&str>, folder: &Path| {
            find(
                &homes,
                &chat(AgentKind::Antigravity, id, started, &[folder]),
                &none,
            )
            .unwrap()
            .map(|f| (f.id, f.main))
        };
        assert_eq!(
            agy(None, app),
            Some((
                "a-1".into(),
                antigravity_transcript(&homes.antigravity, "a-1")
            ))
        );
        assert_eq!(agy(None, other).map(|(id, _)| id).as_deref(), Some("a-2"));
        assert_eq!(agy(Some("a-3"), app), None, "no such conversation");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn times_read_as_seconds() {
        assert_eq!(iso_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_secs("2026-08-26T10:58:45.531Z"), Some(1_787_741_925));
        assert_eq!(iso_secs("2026-08-26T12:58:45+02:00"), Some(1_787_741_925));
        assert_eq!(civil(days_of(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(
            codex_days(1_787_741_925),
            vec!["2026/08/25", "2026/08/26", "2026/08/27"]
        );
    }

    #[test]
    fn a_claude_chat_is_found_with_its_extras() {
        let home = scratch("locate-claude");
        let homes = Homes::under(&home);
        let folder = home.join(".claude/projects/-work-app");
        put(&folder.join("abc-1.jsonl"), "{}\n");
        put(&folder.join("abc-1/tool-results/t1.txt"), "out");
        put(&folder.join("abc-1/subagents/agent-x.jsonl"), "{}\n");
        put(&folder.join("abc-1/other/skip.txt"), "no");
        put(&home.join(".claude/projects/-other/zzz.jsonl"), "{}\n");
        let c = chat(AgentKind::Claude, Some("abc-1"), 0, &[]);
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!(f.main, folder.join("abc-1.jsonl"));
        assert_eq!(f.form, Form::Lines);
        let rels: Vec<&str> = f.extra.iter().map(|(rel, _)| rel.as_str()).collect();
        assert_eq!(rels, ["subagents/agent-x.jsonl", "tool-results/t1.txt"]);
        let gone = chat(AgentKind::Claude, Some("nope"), 0, &[]);
        assert_eq!(find(&homes, &gone, &HashSet::new()).unwrap(), None);
        let bad = chat(AgentKind::Claude, Some("../x"), 0, &[]);
        assert!(find(&homes, &bad, &HashSet::new()).is_err());
    }

    #[test]
    fn listings_read_back_from_a_script() {
        let c = chat(AgentKind::Cursor, Some("c-1"), 0, &[]);
        assert!(script(&c).unwrap().contains("id='c-1'"));
        let out = "M\t100 5\t\t/h/.cursor/chats/a/c-1/store.db\n\
                   W\t7 6\t\t/h/.cursor/chats/a/c-1/store.db-wal\n\
                   X\t2 5\tmeta.json\t/h/.cursor/chats/a/c-1/meta.json\n\
                   X\t2 5\t../evil\t/etc/passwd\n";
        let listing = read_listing(&c, out, &HashSet::new()).unwrap();
        assert_eq!(listing.main.len, 100);
        assert_eq!(listing.wal.unwrap().mtime, 6);
        assert_eq!(listing.extra.len(), 1);
        assert_eq!(read_listing(&c, "", &HashSet::new()), None);

        let work = Path::new("/w");
        let started = iso_secs("2026-08-26T10:58:00Z").unwrap();
        let codex = chat(AgentKind::Codex, None, started, &[work]);
        assert!(script(&codex).unwrap().contains("2026/08/26"));
        let first = r#"{"type":"session_meta","payload":{"id":"x-1","cwd":"/w","timestamp":"2026-08-26T10:58:30Z"}}"#;
        let out =
            format!("C\t9 1\t\t/h/.codex/sessions/2026/08/26/rollout-a-x-1.jsonl\nL\t{first}\n");
        let listing = read_listing(&codex, &out, &HashSet::new()).unwrap();
        assert_eq!(listing.id, "x-1");
        assert!(script(&chat(AgentKind::OpenCode, None, 0, &[])).is_none());
    }

    #[test]
    fn the_scripts_run_in_sh() {
        let home = scratch("locate-script");
        let folder = home.join(".claude/projects/-w");
        put(&folder.join("s-1.jsonl"), "{}\n");
        put(&folder.join("s-1/subagents/a.jsonl"), "{}\n");
        let c = chat(AgentKind::Claude, Some("s-1"), 0, &[]);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(script(&c).unwrap())
            .env("HOME", &home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap();
        let out = String::from_utf8(out.stdout).unwrap();
        let listing = read_listing(&c, &out, &HashSet::new()).unwrap();
        assert_eq!(
            listing.main.path,
            folder.join("s-1.jsonl").to_str().unwrap()
        );
        assert_eq!(listing.main.len, 3);
        assert_eq!(listing.extra[0].0, "subagents/a.jsonl");
    }

    #[test]
    fn an_opencode_chat_is_read_out_of_its_database() {
        let home = scratch("locate-opencode");
        let homes = Homes::under(&home);
        std::fs::create_dir_all(homes.opencode.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&homes.opencode).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             INSERT INTO session VALUES ('ses_a', NULL, '/w/x', 100500);
             INSERT INTO session VALUES ('ses_sub', 'ses_a', '/w/x', 100600);
             INSERT INTO message VALUES ('m1', 'ses_a', 100700, '{\"role\":\"user\"}');
             INSERT INTO part VALUES ('p1', 'm1', 'ses_a', 100701, '{\"text\":\"hi\"}');",
        )
        .unwrap();
        drop(conn);
        let c = chat(AgentKind::OpenCode, None, 100, &[Path::new("/w")]);
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!(f.id, "ses_a");
        let Form::Made(lines) = &f.form else {
            panic!("{:?}", f.form)
        };
        let line: Value =
            serde_json::from_slice(lines.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(line["data"]["role"], "user");
        assert_eq!(line["parts"][0]["data"]["text"], "hi");
        assert_eq!(f.rows[0].0, "opencode/session.json");
    }

    #[test]
    fn a_newer_opencode_chat_is_found_beside_older_ones() {
        let home = scratch("locate-opencode2");
        let homes = Homes::under(&home);
        std::fs::create_dir_all(homes.opencode.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&homes.opencode).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, data TEXT);
             INSERT INTO session VALUES ('ses_old', NULL, '/w/x', 100500);
             INSERT INTO session_v2 VALUES ('ses_old', NULL, '/w/x', 100500);
             INSERT INTO session_v2 VALUES ('ses_new', NULL, '/w/y', 200500);
             INSERT INTO session_message VALUES ('m1', 'ses_new', 'user', 1, '{\"text\":\"hi\"}');",
        )
        .unwrap();
        drop(conn);
        let mut c = chat(AgentKind::OpenCode, None, 200, &[Path::new("/w/y")]);
        c.id = Some("ses_new".into());
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        let Form::Made(lines) = &f.form else {
            panic!("{:?}", f.form)
        };
        let line: Value =
            serde_json::from_slice(lines.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(
            (line["type"].as_str(), line["data"]["text"].as_str()),
            (Some("user"), Some("hi"))
        );
        // By folder and time too.
        c.id = None;
        assert_eq!(
            find(&homes, &c, &HashSet::new()).unwrap().unwrap().id,
            "ses_new"
        );
    }

    #[test]
    fn a_t3_thread_leads_to_its_claude_chat() {
        let home = scratch("locate-t3");
        let mut homes = Homes::under(&home);
        let t3 = home.join(".t3");
        homes.t3 = Some(t3.clone());
        let db = t3.join(crate::t3_agents::DB);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(include_str!("../t3_schema.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO orchestration_v2_projection_threads (thread_id, project_id, title, \
               default_provider, runtime_mode, interaction_mode, created_at, updated_at, payload_json) \
             VALUES ('th-1', 'p', 'T', 'claudeAgent', 'full', 'default', 'a', 'b', '{}');
             INSERT INTO orchestration_v2_projection_runs (run_id, thread_id, ordinal, provider, status, \
               requested_at, payload_json) VALUES ('r1', 'th-1', 1, 'claudeAgent', 'completed', 'a', '{}');",
        )
        .unwrap();
        let missing = columns(&conn, "orchestration_v2_projection_provider_threads")
            .unwrap()
            .is_empty();
        if missing {
            conn.execute_batch(
                "CREATE TABLE orchestration_v2_projection_provider_threads (provider_thread_id TEXT, \
                   thread_id TEXT, provider TEXT, updated_at TEXT, payload_json TEXT);",
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO orchestration_v2_projection_provider_threads (provider_thread_id, thread_id, \
               provider, updated_at, payload_json) VALUES ('pt', 'th-1', 'claudeAgent', 'c', ?1)",
            [r#"{"nativeThreadRef":{"driver":"claudeAgent","nativeId":"cl-9"}}"#],
        )
        .unwrap();
        drop(conn);
        let c = chat(AgentKind::T3, Some("th-1"), 0, &[]);
        // Its Claude chat isn't there yet: the thread's rows alone.
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!((f.agent, f.form.clone()), (AgentKind::T3, Form::Nothing));
        assert_eq!(f.rows.len(), 3);
        put(&home.join(".claude/projects/-w/cl-9.jsonl"), "{}\n");
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!((f.agent, f.id.as_str()), (AgentKind::Claude, "cl-9"));
        let runs: Value = serde_json::from_slice(&f.rows[1].1).unwrap();
        assert_eq!(runs[0]["run_id"], "r1");
    }
}
