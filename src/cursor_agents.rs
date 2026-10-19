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
//!
//! A chat that went idle stays as history (see `History`): those not in
//! agent mode (asking, planning, debugging) until the user deletes them,
//! the others while they are among the latest.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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
/// How long a subagent left running in the background counts as running
/// after its last hook: none says when one stops.
pub const BACKGROUND_HOLD_MS: i64 = 2 * 60_000;
/// The tool through which a chat runs a subagent.
const SUBAGENT_TOOL: &str = "Subagent";
/// How many idle chats a workspace shows at most.
pub const HISTORY_CARDS: usize = 5;
/// The mode of a chat that works on its own, as the database has it; the
/// others ask, plan or debug with the user.
const AGENT_MODE: &str = "agent";
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
    /// Its mode: `agent`, `chat` (asking), `plan`, `debug`, `multitask`.
    pub mode: Option<String>,
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

    /// Whether it works on its own (a chat of unknown mode is taken to),
    /// rather than ask, plan or debug with the user.
    pub fn agent_mode(&self) -> bool {
        self.mode.as_deref().is_none_or(|m| m == AGENT_MODE)
    }

    /// Its mode as the user knows it from Cursor's picker, unless it is an
    /// agent's.
    pub fn mode_name(&self) -> Option<&str> {
        let mode = self.mode.as_deref().filter(|_| !self.agent_mode())?;
        Some(if mode == "chat" { "ask" } else { mode })
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
    /// Whether the chat has a transcript yet. A subagent's never has, nor
    /// a chat's first run before its first answer.
    #[serde(default)]
    pub transcript: bool,
    /// The subagent call it is about, for the subagent tool's hooks.
    #[serde(default)]
    pub call: Option<SubagentCall>,
    /// When the hook ran, in milliseconds since the epoch.
    pub at_ms: i64,
}

/// A chat's call of a subagent. Cursor runs each subagent as a chat of its
/// own, whose hooks say nothing of whose it is and which never stops: only
/// its parent's call says when it ends.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubagentCall {
    /// The tool call's id.
    pub id: String,
    /// What the subagent is to do, in a few words.
    #[serde(default)]
    pub task: Option<String>,
    /// The subagent's chat, once the call returns.
    #[serde(default)]
    pub chat: Option<String>,
    /// The subagent runs on in the background once the call returns.
    #[serde(default)]
    pub background: bool,
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
        let call = (text("tool_name").as_deref() == Some(SUBAGENT_TOOL))
            .then(|| text("tool_use_id"))
            .flatten()
            .map(|id| {
                let input = payload.get("tool_input");
                let output = text("tool_output")
                    .and_then(|o| serde_json::from_str::<Value>(&o).ok())
                    .unwrap_or_default();
                SubagentCall {
                    id,
                    task: input
                        .and_then(|i| i.get("description")?.as_str())
                        .map(str::to_owned),
                    chat: output["agentId"].as_str().map(str::to_owned),
                    background: output["isBackground"].as_bool().unwrap_or(false),
                }
            });
        Some(Self {
            event: text("hook_event_name")?,
            chat: text("conversation_id").or_else(|| text("session_id"))?,
            status: text("status"),
            model: text("model").filter(|m| !m.is_empty()),
            roots,
            prompt,
            transcript: text("transcript_path").is_some(),
            call,
            at_ms,
        })
    }

    /// Whether it is about a tool the chat runs.
    fn is_tool(&self) -> bool {
        matches!(
            self.event.as_str(),
            "preToolUse" | "postToolUse" | "postToolUseFailure"
        )
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
    /// What each of its subagents that run is to do, if known.
    pub subagents: Vec<Option<String>>,
}

/// A subagent heard of: whose it is and whether it still runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Subagent {
    /// Its parent's chat.
    pub parent: String,
    /// What it is to do, known once its call returned.
    pub task: Option<String>,
    /// It runs on in the background after its call returned.
    pub background: bool,
    /// It is over: its call returned, or its parent stopped.
    pub ended: bool,
    /// When it was last heard of, in milliseconds since the epoch.
    pub at_ms: i64,
}

/// What gerdr knows of Cursor's chats: the database's rows, what the
/// hooks said last of each, and their subagents, which their parents'
/// cards show rather than cards of their own.
#[derive(Default)]
pub struct Chats {
    pub rows: HashMap<String, ChatRow>,
    pub hooks: HashMap<String, HookEvent>,
    /// The subagent calls each chat has open, the oldest first.
    pub calls: HashMap<String, Vec<SubagentCall>>,
    /// The subagents heard of, by their chats.
    pub subagents: HashMap<String, Subagent>,
}

impl Chats {
    /// Take in the rows of a fresh read.
    pub fn set_rows(&mut self, rows: Vec<ChatRow>) {
        self.rows = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
    }

    /// Take in what a hook said, keeping what earlier ones said of the
    /// chat's folders, prompt and model when it doesn't; a session's end
    /// forgets its chat's. A subagent's hook only says it still runs.
    pub fn hear(&mut self, mut event: HookEvent) {
        if event.event == "sessionEnd" {
            self.hooks.remove(&event.chat);
            self.calls.remove(&event.chat);
            return;
        }
        if self.hear_subagent(&event) {
            return;
        }
        if let Some(call) = event.call.clone() {
            if event.event == "preToolUse" {
                self.calls.entry(event.chat.clone()).or_default().push(call);
            } else if event.is_tool() {
                self.call_returned(&event.chat, call, event.at_ms);
            }
        }
        if event.event == "stop" {
            // Its calls returned before it stopped, unless it was stopped.
            self.calls.remove(&event.chat);
            self.end_subagents(&event.chat);
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

    /// Take in `event` if it is a subagent's, and say whether it was. A
    /// chat is taken for one when a tool's hook without a transcript is
    /// the first heard of it while another chat has a subagent call open.
    /// While it runs, so does its parent, which hears nothing meanwhile.
    fn hear_subagent(&mut self, event: &HookEvent) -> bool {
        if !self.subagents.contains_key(&event.chat) {
            let first = event.is_tool()
                && !event.transcript
                && !self.hooks.contains_key(&event.chat)
                && !self.rows.contains_key(&event.chat);
            let Some(parent) = first.then(|| self.calling(&event.roots)).flatten() else {
                return false;
            };
            let subagent = Subagent {
                parent,
                task: None,
                background: false,
                ended: false,
                at_ms: event.at_ms,
            };
            self.subagents.insert(event.chat.clone(), subagent);
        }
        if let Some(subagent) = self.subagents.get_mut(&event.chat) {
            subagent.at_ms = subagent.at_ms.max(event.at_ms);
            let parent = self.hooks.get_mut(&subagent.parent);
            if let Some(parent) = parent.filter(|_| !subagent.ended)
                && parent.status() == Some(AgentStatus::Working)
            {
                parent.at_ms = parent.at_ms.max(event.at_ms);
            }
        }
        true
    }

    /// The chat with a subagent call open that one starting in `roots`
    /// likeliest belongs to: one in the same folders, the latest heard.
    fn calling(&self, roots: &[PathBuf]) -> Option<String> {
        self.calls
            .iter()
            .filter(|(_, calls)| !calls.is_empty())
            .map(|(chat, _)| chat)
            .max_by_key(|chat| {
                let hook = self.hooks.get(*chat);
                (
                    hook.is_some_and(|h| h.roots == roots),
                    hook.map_or(0, |h| h.at_ms),
                )
            })
            .cloned()
    }

    /// Take in that `parent`'s subagent `call` returned at `at_ms`: its
    /// subagent is over unless it runs on in the background, and once no
    /// call is open, so are all that don't.
    fn call_returned(&mut self, parent: &str, call: SubagentCall, at_ms: i64) {
        let open = self.calls.entry(parent.to_owned()).or_default();
        let task = open
            .iter()
            .position(|c| c.id == call.id)
            .and_then(|i| open.remove(i).task)
            .or(call.task);
        let none_open = open.is_empty();
        if let Some(chat) = call.chat {
            let subagent = Subagent {
                parent: parent.to_owned(),
                task,
                background: call.background,
                ended: !call.background,
                at_ms,
            };
            self.subagents.insert(chat, subagent);
        }
        if none_open {
            self.calls.remove(parent);
            self.end_subagents(parent);
        }
    }

    /// Take in that the subagents of `parent` not in the background are
    /// over.
    fn end_subagents(&mut self, parent: &str) {
        for subagent in self.subagents.values_mut() {
            if subagent.parent == parent && !subagent.background {
                subagent.ended = true;
            }
        }
    }

    /// What each subagent of `parent` running at `now_ms` is to do: those
    /// its open calls run, and those it left in the background that were
    /// heard of in the last `BACKGROUND_HOLD_MS`.
    fn running_subagents(&self, parent: &str, now_ms: i64) -> Vec<Option<String>> {
        let called = self.calls.get(parent).into_iter().flatten();
        let background = self.subagents.values().filter(|s| {
            s.parent == parent && s.background && !s.ended && now_ms - s.at_ms <= BACKGROUND_HOLD_MS
        });
        called
            .map(|c| c.task.clone())
            .chain(background.map(|s| s.task.clone()))
            .collect()
    }

    /// Whether what is live may change with time alone: a hook's word or a
    /// subagent's may get old.
    pub fn ageing(&self) -> bool {
        !self.hooks.is_empty() || !self.subagents.is_empty()
    }

    /// The chats at `now_ms`, the most recently changed first: those of
    /// the database, and those only hooks know of yet (a first run), named
    /// by their prompt. One runs while a subagent of its runs. What hooks
    /// said long ago is forgotten.
    pub fn all(&mut self, now_ms: i64) -> Vec<CursorChat> {
        self.hooks.retain(|_, h| h.holds(now_ms));
        self.calls.retain(|chat, _| self.hooks.contains_key(chat));
        self.subagents
            .retain(|_, s| now_ms - s.at_ms <= WORKING_HOLD_MS);
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
                let subagents = self.running_subagents(&row.id, now_ms);
                let mut status = merged_status(&row, hook, now_ms);
                if !subagents.is_empty() && status != AgentStatus::Blocked {
                    status = AgentStatus::Working;
                }
                CursorChat {
                    status,
                    model: hook.and_then(|h| h.model.clone()),
                    subagents,
                    row,
                }
            })
            .collect();
        chats.sort_by_key(|c| std::cmp::Reverse(c.row.updated_ms));
        chats
    }
}

/// What gerdr remembers of Cursor's idle chats, in `state.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    /// The chats last seen in a mode other than agent: kept until deleted,
    /// and read whatever their age.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub kept: BTreeSet<String>,
    /// The chats deleted from gerdr, each with when it had last changed
    /// then: one that changes again is back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deleted: BTreeMap<String, i64>,
}

impl History {
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty() && self.deleted.is_empty()
    }

    /// Whether chat `row` was deleted since it last changed.
    pub fn is_deleted(&self, row: &ChatRow) -> bool {
        self.deleted
            .get(&row.id)
            .is_some_and(|at| *at >= row.updated_ms)
    }

    /// Take in the `rows` of a fresh read: keep those not in agent mode,
    /// let go those back in it or gone, and forget deletions of chats no
    /// longer read or changed since. Whether anything changed.
    pub fn update(&mut self, rows: &HashMap<String, ChatRow>) -> bool {
        let before = (self.kept.len(), self.deleted.len());
        self.deleted
            .retain(|id, at| rows.get(id).is_some_and(|row| row.updated_ms <= *at));
        self.kept
            .retain(|id| rows.get(id).is_some_and(|row| !row.agent_mode()));
        let mut added = false;
        for row in rows.values() {
            if !row.agent_mode() && !self.is_deleted(row) {
                added |= self.kept.insert(row.id.clone());
            }
        }
        added || before != (self.kept.len(), self.deleted.len())
    }

    /// Delete chat `row` from gerdr, until it changes again.
    pub fn delete(&mut self, row: &ChatRow) {
        self.kept.remove(&row.id);
        self.deleted.insert(row.id.clone(), row.updated_ms);
    }

    /// The chats to read whatever their age.
    pub fn ids(&self) -> Vec<String> {
        self.kept.iter().cloned().collect()
    }

    /// Out of a workspace's `idle` chats (the most recently changed
    /// first), those it shows as history, `HISTORY_CARDS` at most: the
    /// kept ones before the others, in the same order. Also the kept ones
    /// that don't fit. Deleted ones are neither.
    pub fn pick<'a>(&self, idle: &[&'a CursorChat]) -> (Vec<&'a CursorChat>, Vec<&'a CursorChat>) {
        let (kept, others): (Vec<&CursorChat>, Vec<&CursorChat>) = idle
            .iter()
            .filter(|chat| !self.is_deleted(&chat.row))
            .partition(|chat| self.kept.contains(&chat.row.id));
        let fit = kept.len().min(HISTORY_CARDS);
        let mut shown: Vec<&CursorChat> = kept[..fit].to_vec();
        shown.extend(others.iter().take(HISTORY_CARDS - fit));
        shown.sort_by_key(|chat| std::cmp::Reverse(chat.row.updated_ms));
        (shown, kept[fit..].to_vec())
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
/// path is in one of `project_dirs` or holds one (a chat at the root of a
/// project whose workspaces are its subfolders), short of holding the home
/// folder; `None` for a chat of another project.
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
        !home.is_some_and(|home| home.starts_with(path))
            && project_dirs
                .iter()
                .any(|dir| Some(*dir) != home && (path.starts_with(dir) || dir.starts_with(path)))
    });
    ours.then(|| spots.iter().position(|s| s.cursor)).flatten()
}

/// The chats read from the database at `conn`, given when each was read
/// last and its run's state then (`known`, brought up to date): those
/// changed in the last `RECENT_MS` before `now_ms`, waiting, with news or
/// in `keep`.
pub fn read_rows(
    conn: &rusqlite::Connection,
    now_ms: i64,
    keep: &[String],
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
            json_extract(value, '$.trackedGitRepos'),
            json_extract(value, '$.unifiedMode')
        FROM composerHeaders
        WHERE coalesce(isArchived, 0) = 0 AND coalesce(isSubagent, 0) = 0
          AND (lastUpdatedAt > ?1
               OR json_extract(value, '$.hasBlockingPendingActions')
               OR json_extract(value, '$.hasUnreadMessages')
               OR composerId IN (SELECT value FROM json_each(?2)))",
    )?;
    let keep = serde_json::to_string(keep).unwrap_or_else(|_| "[]".into());
    let mut rows: Vec<ChatRow> = headers
        .query_map(rusqlite::params![now_ms - RECENT_MS, keep], |r| {
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
                mode: r.get(14)?,
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
/// of its own, those in `keep` among them whatever their age, and hand
/// them to `send` whenever they changed. A message on the returned sender
/// asks for a read now, and says which chats to keep reading from then on.
/// Stops once `send` returns false or the sender is dropped.
pub fn watch_db(
    db: PathBuf,
    mut keep: Vec<String>,
    mut send: impl FnMut(Vec<ChatRow>) -> bool + Send + 'static,
) -> mpsc::Sender<Vec<String>> {
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
                    match read_if_changed(c, &mut version, &keep, &mut known) {
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
                Ok(mut ids) => {
                    while let Ok(newer) = poked.try_recv() {
                        ids = newer;
                    }
                    if ids != keep {
                        keep = ids;
                        // Read again even if nothing was written.
                        version = None;
                    }
                }
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
    keep: &[String],
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
    let rows = read_rows(conn, now_ms(), keep, known)?;
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
                if state.projects.cursor_history.update(&state.cursor.rows) {
                    state.cursor_history_changed();
                }
                state.cursor_changed();
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("cursor's agents: {err}");
        }
        let keep = self.projects.cursor_history.ids();
        self.cursor_poke = Some(watch_db(db, keep, move |rows| tx.send(rows).is_ok()));
        match hook_socket(&self.socket_name).map(|path| (bind_hook_socket(&path), path)) {
            Some((Ok(socket), path)) => {
                let source = Generic::new(socket, Interest::READ, Mode::Level);
                let inserted = self.loop_handle.insert_source(source, |_, socket, state| {
                    for event in take_events(socket) {
                        state.cursor.hear(event);
                    }
                    state.cursor_changed();
                    state.read_cursor_db();
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
            if state.cursor.ageing() {
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
        let (idle, live) = self
            .cursor
            .all(now_ms())
            .into_iter()
            .partition(|chat| chat.status == AgentStatus::Idle);
        self.cursor_chats = live;
        // A chat only hooks know of goes once idle: there is nothing to
        // show it by.
        self.cursor_idle = idle
            .into_iter()
            .filter(|chat| self.cursor.rows.contains_key(&chat.row.id))
            .collect();
        self.notify_agents();
    }

    /// Ask for Cursor's database to be read now.
    fn read_cursor_db(&self) {
        if let Some(poke) = &self.cursor_poke {
            let _ = poke.send(self.projects.cursor_history.ids());
        }
    }

    /// Save what changed in the history and read the chats it keeps.
    fn cursor_history_changed(&mut self) {
        self.projects.save();
        self.read_cursor_db();
    }

    /// Delete idle Cursor chat `id` from gerdr: its card goes, until the
    /// chat changes again.
    pub fn delete_cursor_chat(&mut self, id: &str) {
        let Some(chat) = self.cursor_idle.iter().find(|chat| chat.row.id == id) else {
            return;
        };
        self.projects.cursor_history.delete(&chat.row);
        self.cursor_history_changed();
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

    /// The idle Cursor chats under workspace `w` as history (see
    /// `History::pick`): those shown, and the kept ones that don't fit.
    pub fn cursor_history(&self, w: usize) -> (Vec<&CursorChat>, Vec<&CursorChat>) {
        let idle: Vec<&CursorChat> = self
            .cursor_idle
            .iter()
            .filter(|chat| self.cursor_chat_place(chat) == Some(w))
            .collect();
        self.projects.cursor_history.pick(&idle)
    }

    /// The cards of the Cursor chats under workspace `w`: those doing
    /// something, then its history, which can be deleted.
    pub fn cursor_cards(&self, w: usize) -> Vec<crate::chrome::AgentView> {
        let live = self
            .cursor_chats
            .iter()
            .filter(|chat| self.cursor_chat_place(chat) == Some(w))
            .map(crate::chrome::AgentView::cursor);
        let history = self.cursor_history(w).0.into_iter().map(|chat| {
            let mut card = crate::chrome::AgentView::cursor(chat);
            card.deletable = true;
            card
        });
        live.chain(history).collect()
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

    /// The chats doing something or with news.
    fn live(chats: &mut Chats, now_ms: i64) -> Vec<CursorChat> {
        let all = chats.all(now_ms).into_iter();
        all.filter(|c| c.status != AgentStatus::Idle).collect()
    }

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
            transcript: true,
            call: None,
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
        let ids: Vec<_> = live(&mut chats, 6_000)
            .into_iter()
            .map(|c| c.row.id)
            .collect();
        assert_eq!(ids, ["news", "runs"]);
        // A hook wakes an idle one; its session's end forgets it.
        let mut start = hook("preToolUse", None, 5_500);
        start.chat = "idle".into();
        chats.hear(start.clone());
        assert_eq!(live(&mut chats, 6_000).len(), 3);
        start.event = "sessionEnd".into();
        chats.hear(start);
        assert_eq!(live(&mut chats, 6_000).len(), 2);
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
        let new = live(&mut chats, 6_000)
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
        let new = live(&mut chats, 6_000)
            .into_iter()
            .find(|c| c.row.id == "new")
            .unwrap();
        assert_eq!(new.row.name.as_deref(), Some("Tests"));
        assert_eq!(new.row.paths, [PathBuf::from("/src/app")]);
        // What hooks said long ago goes.
        live(&mut chats, 5_600 + WORKING_HOLD_MS + 1);
        assert!(chats.hooks.is_empty());
    }

    /// A tool's hook of chat `chat` at `at_ms`, about subagent call `call`
    /// if any; a subagent's has no transcript.
    fn tool(event: &str, chat: &str, call: Option<SubagentCall>, at_ms: i64) -> HookEvent {
        HookEvent {
            chat: chat.into(),
            transcript: call.is_some() || !chat.starts_with("sub"),
            call,
            ..hook(event, None, at_ms)
        }
    }

    fn call(id: &str, task: &str) -> SubagentCall {
        SubagentCall {
            id: id.into(),
            task: Some(task.into()),
            ..SubagentCall::default()
        }
    }

    fn returned(id: &str, chat: &str, background: bool) -> SubagentCall {
        SubagentCall {
            id: id.into(),
            chat: Some(chat.into()),
            background,
            ..SubagentCall::default()
        }
    }

    #[test]
    fn subagent_calls_are_read_from_hook_input() {
        // As Cursor 3.16 sends them.
        let started = serde_json::json!({
            "hook_event_name": "preToolUse",
            "conversation_id": "c1",
            "tool_name": "Subagent",
            "tool_input": { "description": "Audit deck", "prompt": "…" },
            "tool_use_id": "call_1\nfc_1",
            "transcript_path": "/home/u/.cursor/c1.jsonl",
        });
        let event = HookEvent::from_payload(&started, 7).unwrap();
        assert!(event.transcript);
        assert_eq!(event.call, Some(call("call_1\nfc_1", "Audit deck")));
        let ended = serde_json::json!({
            "hook_event_name": "postToolUse",
            "conversation_id": "c1",
            "tool_name": "Subagent",
            "tool_use_id": "call_1\nfc_1",
            "tool_output": "{\"status\":\"success\",\"agentId\":\"s1\",\"isBackground\":false}",
        });
        let call = HookEvent::from_payload(&ended, 7).unwrap().call.unwrap();
        assert_eq!((call.chat.as_deref(), call.background), (Some("s1"), false));
        // Another tool's say nothing of subagents; a subagent's have no
        // transcript.
        let other = serde_json::json!({
            "hook_event_name": "preToolUse",
            "conversation_id": "s1",
            "tool_name": "Read",
            "tool_use_id": "call_2",
            "transcript_path": null,
        });
        let event = HookEvent::from_payload(&other, 7).unwrap();
        assert_eq!((event.call, event.transcript), (None, false));
    }

    #[test]
    fn subagents_show_on_their_parents_card_while_their_calls_run() {
        let mut chats = Chats::default();
        chats.hear(hook("beforeSubmitPrompt", None, 1_000));
        chats.hear(tool("preToolUse", "c1", Some(call("t1", "Audit")), 1_100));
        chats.hear(tool("preToolUse", "c1", Some(call("t2", "Review")), 1_200));
        for (sub, at) in [("sub1", 1_300), ("sub2", 1_400), ("sub1", 1_500)] {
            chats.hear(tool("preToolUse", sub, None, at));
        }
        let shown = live(&mut chats, 2_000);
        assert_eq!(shown.len(), 1, "no cards of their own");
        assert_eq!(shown[0].row.id, "c1");
        assert_eq!(shown[0].status, AgentStatus::Working);
        assert_eq!(
            shown[0].subagents,
            [Some("Audit".into()), Some("Review".into())]
        );
        // Their hooks keep the parent running, which hears nothing itself.
        assert_eq!(chats.hooks["c1"].at_ms, 1_500);
        // One returns; a late hook of its doesn't bring it back.
        chats.hear(tool(
            "postToolUse",
            "c1",
            Some(returned("t1", "sub1", false)),
            2_100,
        ));
        chats.hear(tool("postToolUse", "sub1", None, 2_150));
        let shown = live(&mut chats, 2_200);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].subagents, [Some("Review".into())]);
        // The parent stops: none runs, and the subagents never stopped.
        chats.hear(hook("stop", Some("completed"), 3_000));
        let shown = live(&mut chats, 3_100);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].status, AgentStatus::Done);
        assert!(shown[0].subagents.is_empty());
        chats.hear(tool("postToolUse", "sub2", None, 3_200));
        assert_eq!(live(&mut chats, 3_300).len(), 1);
        assert!(chats.calls.is_empty());
    }

    #[test]
    fn a_subagent_in_the_background_runs_while_heard_of() {
        let mut chats = Chats::default();
        chats.hear(hook("beforeSubmitPrompt", None, 1_000));
        chats.hear(tool("preToolUse", "c1", Some(call("t1", "Watch")), 1_100));
        chats.hear(tool(
            "postToolUse",
            "c1",
            Some(returned("t1", "sub1", true)),
            1_200,
        ));
        chats.hear(hook("stop", Some("completed"), 1_300));
        // Its row is written once it stops.
        let mut news = row("c1");
        news.unread = true;
        chats.set_rows(vec![news]);
        chats.hear(tool("preToolUse", "sub1", None, 1_400));
        let shown = live(&mut chats, 1_500);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].status, AgentStatus::Working);
        assert_eq!(shown[0].subagents, [Some("Watch".into())]);
        assert!(chats.ageing());
        let quiet = live(&mut chats, 1_400 + BACKGROUND_HOLD_MS + 1);
        assert_eq!(quiet[0].status, AgentStatus::Done);
        assert!(quiet[0].subagents.is_empty());
    }

    #[test]
    fn only_a_chat_first_heard_by_a_tool_without_transcript_is_a_subagent() {
        let mut chats = Chats::default();
        chats.hear(tool("preToolUse", "c1", Some(call("t1", "Audit")), 1_000));
        // A chat the user starts meanwhile says so first.
        let mut other = hook("sessionStart", None, 1_100);
        other.chat = "sub-not".into();
        chats.hear(other);
        chats.hear(tool("preToolUse", "sub-not", None, 1_200));
        // One already running, when gerdr started, has a transcript.
        chats.hear(tool("preToolUse", "c2", None, 1_300));
        let mut ids: Vec<_> = live(&mut chats, 2_000)
            .into_iter()
            .map(|c| c.row.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["c1", "c2", "sub-not"]);
        // With no call open, a chat without transcript is one of its own.
        chats.hear(tool(
            "postToolUseFailure",
            "c1",
            Some(call("t1", "Audit")),
            2_100,
        ));
        chats.hear(tool("preToolUse", "sub3", None, 2_200));
        assert!(live(&mut chats, 2_300).iter().any(|c| c.row.id == "sub3"));
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
                transcript: false,
                call: None,
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
        // At the root of a project whose workspaces are its subfolders.
        let subfolders = [Path::new("/home/u/app/api"), Path::new("/home/u/app/web")];
        let paths = [PathBuf::from("/home/u/app")];
        assert_eq!(place(&paths, &lone, &subfolders, Some(home)), Some(0));
        // A folder holding home holds every project.
        for path in ["/home/u", "/home", "/"] {
            let paths = [PathBuf::from(path)];
            assert_eq!(place(&paths, &lone, &subfolders, Some(home)), None);
        }
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
        let mut rows = read_rows(&conn, now, &[], &mut known).unwrap();
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
        assert_eq!(a.mode, None);
        // A kept chat is read whatever its age, with its mode.
        conn.execute(
            "UPDATE composerHeaders SET value = '{\"unifiedMode\":\"plan\"}' WHERE composerId = 'old'",
            [],
        )
        .unwrap();
        let rows = read_rows(&conn, now, &["old".into(), "gone".into()], &mut known).unwrap();
        let old = rows.iter().find(|r| r.id == "old").unwrap();
        assert_eq!(old.mode.as_deref(), Some("plan"));
        assert_eq!(old.mode_name(), Some("plan"));
        // An archived one isn't.
        assert!(rows.iter().all(|r| r.id != "gone"));
    }

    fn moded(id: &str, mode: &str, updated_ms: i64) -> ChatRow {
        ChatRow {
            mode: Some(mode.into()),
            updated_ms,
            ..row(id)
        }
    }

    fn idle(row: ChatRow) -> CursorChat {
        CursorChat {
            row,
            status: AgentStatus::Idle,
            model: None,
            subagents: Vec::new(),
        }
    }

    #[test]
    fn chats_not_in_agent_mode_are_kept_until_deleted() {
        let mut history = History::default();
        let rows: HashMap<String, ChatRow> = [
            moded("plan", "plan", 3_000),
            moded("ask", "chat", 2_000),
            moded("agent", "agent", 1_000),
            row("unknown"),
        ]
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect();
        assert!(history.update(&rows));
        assert_eq!(history.ids(), ["ask", "plan"]);
        assert!(!history.update(&rows));
        // Deleted: no longer kept, until it changes again.
        history.delete(&rows["plan"]);
        assert!(history.is_deleted(&rows["plan"]));
        assert!(!history.update(&rows));
        assert_eq!(history.ids(), ["ask"]);
        let mut changed = rows.clone();
        changed.insert("plan".into(), moded("plan", "plan", 3_001));
        assert!(history.update(&changed));
        assert_eq!(history.ids(), ["ask", "plan"]);
        assert!(history.deleted.is_empty());
        // Back in agent mode, or gone from the database: let go.
        changed.insert("ask".into(), moded("ask", "agent", 2_001));
        changed.remove("plan");
        assert!(history.update(&changed));
        assert!(history.is_empty());
        // Survives state.json.
        history.delete(&rows["agent"]);
        let json = serde_json::to_string(&history).unwrap();
        assert_eq!(serde_json::from_str::<History>(&json).unwrap(), history);
    }

    #[test]
    fn history_shows_kept_chats_first_and_counts_the_rest() {
        let chats: Vec<CursorChat> = (0..9)
            .map(|n| {
                let mode = if n % 2 == 0 { "agent" } else { "plan" };
                idle(moded(&format!("c{n}"), mode, 10_000 - n))
            })
            .collect();
        let idle: Vec<&CursorChat> = chats.iter().collect();
        let ids = |chats: &[&CursorChat]| -> Vec<String> {
            chats.iter().map(|c| c.row.id.clone()).collect()
        };
        let mut history = History::default();
        // Nothing kept: the latest five.
        let (shown, more) = history.pick(&idle);
        assert_eq!(ids(&shown), ["c0", "c1", "c2", "c3", "c4"]);
        assert!(more.is_empty());
        // Four kept: those, and the latest other, newest first.
        let rows = chats
            .iter()
            .map(|c| (c.row.id.clone(), c.row.clone()))
            .collect();
        history.update(&rows);
        let (shown, more) = history.pick(&idle);
        assert_eq!(ids(&shown), ["c0", "c1", "c3", "c5", "c7"]);
        assert!(more.is_empty());
        // Seven kept: five shown, two more; deleted ones neither.
        for n in [2, 4, 6] {
            history.kept.insert(format!("c{n}"));
        }
        history.delete(&chats[0].row);
        history.kept.insert("c0".into());
        let (shown, more) = history.pick(&idle);
        assert_eq!(ids(&shown), ["c1", "c2", "c3", "c4", "c5"]);
        assert_eq!(ids(&more), ["c6", "c7"]);
    }
}
