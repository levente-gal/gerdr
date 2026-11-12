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
//! the others while they are among the latest. A chat done with news can
//! be deleted too: Cursor may never mark it read. A chat closed while it
//! runs is hidden, and deleted as its run leaves it once that ends.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Read,
    os::unix::net::UnixDatagram,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::herdr::{AgentStatus, runtime_dir};

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
/// The tool through which a chat runs a subagent, by the names Cursor
/// 3.16 has given it.
const SUBAGENT_TOOLS: [&str; 2] = ["Subagent", "Task"];
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
    crate::ai_work::now_ms() as i64
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
    /// For a subagent's chat, the call that started it.
    pub spawn: Option<Spawn>,
}

/// The subagent call a subagent's chat was started by, as the database
/// has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawn {
    /// The parent's chat.
    pub parent: String,
    /// The call's id, as its hooks have it.
    pub call: String,
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
        let call = text("tool_name")
            .is_some_and(|tool| SUBAGENT_TOOLS.contains(&tool.as_str()))
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
    /// Its subagents that run.
    pub subagents: Vec<SubagentRun>,
}

/// A subagent that runs, as its parent's card shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubagentRun {
    /// Its chat, once known.
    pub chat: Option<String>,
    /// What it is to do, if known.
    pub task: Option<String>,
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
    /// When it was first and last heard of, in milliseconds since the
    /// epoch.
    pub since_ms: i64,
    pub at_ms: i64,
}

/// What gerdr knows of Cursor's chats: the database's rows, what the
/// hooks said last of each, and their subagents, which their parents'
/// cards show rather than cards of their own.
#[derive(Default)]
pub struct Chats {
    /// The database's rows but its subagents'.
    pub rows: HashMap<String, ChatRow>,
    /// The calls that started the subagents the database has, by their
    /// chats.
    pub spawns: HashMap<String, Spawn>,
    pub hooks: HashMap<String, HookEvent>,
    /// The subagent calls each chat has open, the oldest first.
    pub calls: HashMap<String, Vec<SubagentCall>>,
    /// The subagents heard of, by their chats.
    pub subagents: HashMap<String, Subagent>,
}

impl Chats {
    /// Take in the rows of a fresh read.
    pub fn set_rows(&mut self, rows: Vec<ChatRow>) {
        let (spawned, rows): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| r.spawn.is_some());
        self.spawns = spawned
            .into_iter()
            .filter_map(|r| Some((r.id, r.spawn?)))
            .collect();
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
    /// chat is one the database says is, or taken for one when a tool's
    /// hook without a transcript is the first heard of it while another
    /// chat has a subagent call open. While it runs, so does its parent,
    /// which hears nothing meanwhile.
    fn hear_subagent(&mut self, event: &HookEvent) -> bool {
        if !self.subagents.contains_key(&event.chat) {
            let first = event.is_tool()
                && !event.transcript
                && !self.hooks.contains_key(&event.chat)
                && !self.rows.contains_key(&event.chat);
            let spawned = self.spawns.get(&event.chat).map(|s| s.parent.clone());
            let Some(parent) = spawned.or_else(|| first.then(|| self.calling(&event.roots))?)
            else {
                return false;
            };
            let subagent = Subagent {
                parent,
                task: None,
                background: false,
                ended: false,
                since_ms: event.at_ms,
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
            let since_ms = self.subagents.get(&chat).map_or(at_ms, |s| s.since_ms);
            let subagent = Subagent {
                parent: parent.to_owned(),
                task,
                background: call.background,
                ended: !call.background,
                since_ms,
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

    /// The subagents of `parent` running at `now_ms`: those its open
    /// calls run, and those it left in the background that were heard of
    /// in the last `BACKGROUND_HOLD_MS`. A call's chat is the one the
    /// database says it started, else the next of those heard of running
    /// for it that the database doesn't place, in the order they started.
    fn running_subagents(&self, parent: &str, now_ms: i64) -> Vec<SubagentRun> {
        let spawned = |call: &str| {
            self.spawns
                .iter()
                .find(|(_, s)| s.parent == parent && s.call == call)
                .map(|(chat, _)| chat.clone())
        };
        let mut heard: Vec<(&String, &Subagent)> = self
            .subagents
            .iter()
            .filter(|(chat, s)| {
                s.parent == parent && !s.background && !s.ended && !self.spawns.contains_key(*chat)
            })
            .collect();
        heard.sort_by_key(|(chat, s)| (s.since_ms, *chat));
        let mut heard = heard.into_iter().map(|(chat, _)| chat.clone());
        let called = self.calls.get(parent).into_iter().flatten();
        let called: Vec<SubagentRun> = called
            .map(|c| SubagentRun {
                chat: spawned(&c.id).or_else(|| heard.next()),
                task: c.task.clone(),
            })
            .collect();
        let background = self.subagents.iter().filter(|(_, s)| {
            s.parent == parent && s.background && !s.ended && now_ms - s.at_ms <= BACKGROUND_HOLD_MS
        });
        called
            .into_iter()
            .chain(background.map(|(chat, s)| SubagentRun {
                chat: Some(chat.clone()),
                task: s.task.clone(),
            }))
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
    /// The chats closed while they ran: hidden, and deleted once their run
    /// is over (see `settle`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub closed: BTreeSet<String>,
}

impl History {
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty() && self.deleted.is_empty() && self.closed.is_empty()
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
        let before = (self.kept.len(), self.deleted.len(), self.closed.len());
        self.deleted
            .retain(|id, at| rows.get(id).is_some_and(|row| row.updated_ms <= *at));
        self.closed.retain(|id| rows.contains_key(id));
        self.kept
            .retain(|id| rows.get(id).is_some_and(|row| !row.agent_mode()));
        let mut added = false;
        for row in rows.values() {
            if !row.agent_mode() && !self.is_deleted(row) {
                added |= self.kept.insert(row.id.clone());
            }
        }
        added || before != (self.kept.len(), self.deleted.len(), self.closed.len())
    }

    /// Whether chat `chat` is hidden: closed while it ran, or done and
    /// deleted since it last changed; `hook` is what a hook said of it
    /// last, which may tell of a run ended since that the database doesn't
    /// know yet.
    pub fn hides(&self, chat: &CursorChat, hook: Option<&HookEvent>) -> bool {
        self.closed.contains(&chat.row.id)
            || chat.status == AgentStatus::Done
                && self.is_deleted(&chat.row)
                && hook.is_none_or(|h| h.at_ms <= chat.row.updated_ms)
    }

    /// Close chat `row`, which runs: hidden until `settle` deletes it.
    pub fn close(&mut self, row: &ChatRow) {
        self.kept.remove(&row.id);
        self.closed.insert(row.id.clone());
    }

    /// Delete the closed chats among `chats` whose run is over and known
    /// to the database (`hooks` says what a hook said of each last), as
    /// the run left them. Whether any was.
    pub fn settle(&mut self, chats: &[CursorChat], hooks: &HashMap<String, HookEvent>) -> bool {
        let over: Vec<&ChatRow> = chats
            .iter()
            .filter(|chat| self.closed.contains(&chat.row.id))
            .filter(|chat| !matches!(chat.status, AgentStatus::Working | AgentStatus::Blocked))
            .filter(|chat| {
                let hook = hooks.get(&chat.row.id);
                hook.is_none_or(|h| h.at_ms <= chat.row.updated_ms)
            })
            .map(|chat| &chat.row)
            .collect();
        for row in &over {
            self.closed.remove(&row.id);
            self.delete(row);
        }
        !over.is_empty()
    }

    /// Delete chat `row` from gerdr, until it changes again.
    pub fn delete(&mut self, row: &ChatRow) {
        self.kept.remove(&row.id);
        self.closed.remove(&row.id);
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
            .filter(|chat| !self.is_deleted(&chat.row) && !self.closed.contains(&chat.row.id))
            .partition(|chat| self.kept.contains(&chat.row.id));
        let fit = kept.len().min(HISTORY_CARDS);
        let mut shown: Vec<&CursorChat> = kept[..fit].to_vec();
        shown.extend(others.iter().take(HISTORY_CARDS - fit));
        shown.sort_by_key(|chat| std::cmp::Reverse(chat.row.updated_ms));
        (shown, kept[fit..].to_vec())
    }
}

/// A workspace as placing a chat sees it: its folder, the other worktrees
/// of its folder's repository and whether it is the one chats of the
/// project no folder holds go under (the project's Cursor workspace): that
/// one holds none by its folder.
pub struct Spot<'a> {
    pub dir: &'a Path,
    pub worktrees: Vec<&'a Path>,
    pub fallback: bool,
}

/// Which of `spots` a chat working in `paths` goes under: the one whose
/// folder, or another worktree of whose repository, holds the likeliest
/// path deepest (one in the home folder holds everything, so it doesn't
/// count), else the fallback workspace, if the path is in one of
/// `project_dirs` or holds one (a chat at the root of a project whose
/// workspaces are its subfolders), short of holding the home folder;
/// `None` for a chat of another project.
pub fn place(
    paths: &[PathBuf],
    spots: &[Spot],
    project_dirs: &[&Path],
    home: Option<&Path>,
) -> Option<usize> {
    for path in paths {
        // Its own folder before another worktree as deep.
        let holding = spots
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.fallback && Some(s.dir) != home)
            .flat_map(|(i, s)| {
                let own = std::iter::once((i, s.dir, true));
                own.chain(s.worktrees.iter().map(move |dir| (i, *dir, false)))
            })
            .filter(|(_, dir, _)| path.starts_with(dir))
            .max_by_key(|(_, dir, own)| (dir.components().count(), *own));
        if let Some((i, _, _)) = holding {
            return Some(i);
        }
    }
    let ours = paths.iter().any(|path| {
        !home.is_some_and(|home| home.starts_with(path))
            && project_dirs
                .iter()
                .any(|dir| Some(*dir) != home && (path.starts_with(dir) || dir.starts_with(path)))
    });
    ours.then(|| spots.iter().position(|s| s.fallback))
        .flatten()
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
            json_extract(value, '$.unifiedMode'),
            json_extract(value, '$.subagentInfo.parentComposerId'),
            json_extract(value, '$.subagentInfo.toolCallId')
        FROM composerHeaders
        WHERE coalesce(isArchived, 0) = 0
          AND (coalesce(isSubagent, 0) = 0
               AND (lastUpdatedAt > ?1
                    OR json_extract(value, '$.hasBlockingPendingActions')
                    OR json_extract(value, '$.hasUnreadMessages')
                    OR composerId IN (SELECT value FROM json_each(?2)))
               OR isSubagent AND createdAt > ?1)",
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
                spawn: match (r.get(15)?, r.get(16)?) {
                    (Some(parent), Some(call)) => Some(Spawn { parent, call }),
                    _ => None,
                },
                paths,
            })
        })?
        // One odd row (a field of another type) skips that chat, not all.
        .filter_map(|row| {
            row.inspect_err(|err| {
                static TOLD: std::sync::Once = std::sync::Once::new();
                TOLD.call_once(|| tracing::warn!("cursor chats: a row skipped: {err}"));
            })
            .ok()
        })
        .collect();
    let mut status = conn.prepare_cached(
        "SELECT json_extract(value, '$.status') FROM cursorDiskKV WHERE key = ?1",
    )?;
    // A subagent's state is its parent's call's.
    for row in rows.iter_mut().filter(|r| r.spawn.is_none()) {
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
                // A status of another type is none.
                found.next().and_then(Result::ok).flatten()
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
pub(crate) fn open_read_only(db: &Path) -> rusqlite::Result<rusqlite::Connection> {
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
        match serde_json::from_slice::<HookEvent>(&buf[..n]) {
            // Any local program can send one: a time ahead of now would
            // hold its word for ever.
            Ok(mut event) => {
                event.at_ms = event.at_ms.clamp(0, now_ms());
                events.push(event);
            }
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
    // Cursor's agent CLI runs the same hooks, but herdr shows it already,
    // as the agent of its pane.
    let under_cli = crate::launch::lineage(std::process::id())
        .into_iter()
        .filter_map(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok())
        .any(|cmdline| is_cli_agent(&crate::launch::cmdline_args(&cmdline)));
    if under_cli {
        return;
    }
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

/// Whether a process running `args` is Cursor's agent CLI started in a
/// terminal: its script runs node on the `index.js` of a version of it.
/// Not one of the workers Cursor runs its own chats in, by the same CLI.
fn is_cli_agent(args: &[String]) -> bool {
    let cli = args
        .iter()
        .any(|arg| arg.contains("/cursor-agent/versions/") && arg.ends_with("/index.js"));
    cli && !args.iter().any(|arg| arg == "worker")
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
    Path::new(live_path(program))
        .file_name()
        .is_some_and(|name| name == "gerdr")
}

/// `path` as /proc names a program run from it: without the " (deleted)"
/// it gets once the file is built again under the running program.
fn live_path(path: &str) -> &str {
    path.strip_suffix(" (deleted)").unwrap_or(path)
}

/// Add this gerdr to Cursor's hooks, in `~/.cursor/hooks.json`, unless it
/// is there already.
pub fn install_hooks(home: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let command = format!(
        "{} {HOOK_COMMAND}",
        crate::desktop::shell_quote(live_path(&exe.to_string_lossy()))
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
    crate::projects::write_atomic(&path, merged.as_bytes())
        .map_err(|err| format!("{}: {err}", path.display()))
}

impl crate::state::CursorWatch {
    /// Ask for Cursor's database to be read now.
    pub(crate) fn read_db(&self, projects: &crate::projects::Store) {
        if let Some(poke) = &self.poke {
            let _ = poke.send(projects.cursor_history.ids());
        }
    }

    /// Stop hearing Cursor's hooks: gerdr quits.
    pub fn close_socket(&mut self) {
        if let Some(path) = self.socket.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl crate::Gerdr {
    /// Stop hearing Cursor's hooks: gerdr quits.
    pub fn close_cursor_socket(&mut self) {
        self.cursor.close_socket();
    }

    /// Follow what gerdr's Cursor's chats do: read its database off the
    /// event loop, hear its hooks, and look again every `DB_INTERVAL`, for
    /// a hook's word gets old.
    pub fn watch_cursor_agents(&mut self) {
        use smithay::reexports::calloop::{
            channel,
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
                state.cursor.reads.set_rows(rows);
                if state
                    .projects
                    .cursor_history
                    .update(&state.cursor.reads.rows)
                {
                    state.cursor_history_changed();
                }
                state.cursor_changed();
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("cursor's agents: {err}");
        }
        let keep = self.projects.cursor_history.ids();
        self.cursor.poke = Some(watch_db(db, keep, move |rows| tx.send(rows).is_ok()));
        match hook_socket(&self.socket_name) {
            Some(path) => {
                if self.listen_cursor_hooks(&path).is_some() {
                    self.cursor.socket = Some(path);
                }
            }
            None => tracing::warn!("cursor's hooks: no XDG_RUNTIME_DIR"),
        }
        let timer = Timer::from_duration(DB_INTERVAL);
        let tick = |_, _: &mut (), state: &mut crate::Gerdr| {
            if state.cursor.reads.ageing() {
                state.cursor_changed();
            }
            TimeoutAction::ToDuration(DB_INTERVAL)
        };
        if let Err(err) = self.loop_handle.insert_source(timer, tick) {
            tracing::warn!("cursor's agents: {err}");
        }
    }

    /// Hear Cursor's hooks at `path` (see `hook_socket`): gerdr's own, or
    /// a server's of apps kept running (see `keep`). `None` if it can't.
    pub fn listen_cursor_hooks(
        &mut self,
        path: &Path,
    ) -> Option<smithay::reexports::calloop::RegistrationToken> {
        use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
        let socket = bind_hook_socket(path)
            .inspect_err(|err| tracing::warn!("cursor's hooks, {}: {err}", path.display()))
            .ok()?;
        let source = Generic::new(socket, Interest::READ, Mode::Level);
        self.loop_handle
            .insert_source(source, |_, socket, state| {
                for event in take_events(socket) {
                    state.cursor.reads.hear(event);
                }
                state.cursor_changed();
                state.cursor.read_db(&state.projects);
                Ok(PostAction::Continue)
            })
            .inspect_err(|err| tracing::warn!("cursor's hooks: {err}"))
            .ok()
    }

    /// Take in what Cursor's chats do now, and notify of what is news.
    fn cursor_changed(&mut self) {
        let all = self.cursor.reads.all(now_ms());
        let history = &mut self.projects.cursor_history;
        if history.settle(&all, &self.cursor.reads.hooks) {
            self.projects.save();
        }
        let (idle, live): (Vec<CursorChat>, Vec<CursorChat>) = all
            .into_iter()
            .partition(|chat| chat.status == AgentStatus::Idle);
        let (history, hooks) = (&self.projects.cursor_history, &self.cursor.reads.hooks);
        self.cursor.chats = live
            .into_iter()
            .filter(|chat| !history.hides(chat, hooks.get(&chat.row.id)))
            .collect();
        // A chat only hooks know of goes once idle: there is nothing to
        // show it by.
        self.cursor.idle = idle
            .into_iter()
            .filter(|chat| self.cursor.reads.rows.contains_key(&chat.row.id))
            .collect();
        self.notify_agents();
    }

    /// Save what changed in the history and read the chats it keeps.
    fn cursor_history_changed(&mut self) {
        self.projects.save();
        self.cursor.read_db(&self.projects);
    }

    /// Delete Cursor chat `id` from gerdr: its card goes, until the chat
    /// changes again. One that runs goes now, and is deleted as its run
    /// leaves it.
    pub fn delete_cursor_chat(&mut self, id: &str) {
        let Some(chat) = self
            .cursor
            .idle
            .iter()
            .chain(&self.cursor.chats)
            .find(|chat| chat.row.id == id && self.cursor.reads.rows.contains_key(id))
        else {
            return;
        };
        let history = &mut self.projects.cursor_history;
        match chat.status {
            AgentStatus::Working | AgentStatus::Blocked => history.close(&chat.row),
            _ => history.delete(&chat.row),
        }
        self.cursor_history_changed();
        self.cursor_changed();
    }

    /// The workspace of the shown project Cursor chat `chat` goes under
    /// (see `place`).
    pub fn cursor_chat_place(&self, chat: &CursorChat) -> Option<usize> {
        let spots: Vec<Spot> = self
            .workspaces
            .iter()
            .map(|ws| Spot {
                dir: &ws.dir,
                worktrees: self.git.status.get(&ws.dir).map_or_else(Vec::new, |git| {
                    git.worktrees.iter().map(|w| w.path.as_path()).collect()
                }),
                fallback: ws.kind == crate::WorkspaceKind::Cursor,
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
            .cursor
            .idle
            .iter()
            .filter(|chat| self.cursor_chat_place(chat) == Some(w))
            .collect();
        self.projects.cursor_history.pick(&idle)
    }

    /// The cards of the Cursor chats under workspace `w`: those doing
    /// something, then its history; those in the database can be deleted.
    pub fn cursor_cards(&self, w: usize) -> Vec<crate::chrome::AgentView> {
        let live = self
            .cursor
            .chats
            .iter()
            .filter(|chat| self.cursor_chat_place(chat) == Some(w))
            .map(|chat| {
                let mut card = crate::chrome::AgentView::cursor(chat);
                card.closable = self.cursor.reads.rows.contains_key(&chat.row.id);
                card
            });
        let history = self.cursor_history(w).0.into_iter().map(|chat| {
            let mut card = crate::chrome::AgentView::cursor(chat);
            card.closable = true;
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
        self.cursor
            .chats
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
                    seen: self.notices.host_focused && shown,
                    finished: None,
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
        // Or by its other name.
        let mut task = started.clone();
        task["tool_name"] = "Task".into();
        let event = HookEvent::from_payload(&task, 7).unwrap();
        assert_eq!(event.call.map(|c| c.task), Some(Some("Audit deck".into())));
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
        // Their chats, in the order they started.
        assert_eq!(
            shown[0].subagents,
            [run(Some("sub1"), "Audit"), run(Some("sub2"), "Review")]
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
        assert_eq!(shown[0].subagents, [run(Some("sub2"), "Review")]);
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

    /// A subagent running chat `chat`, to do `task`.
    fn run(chat: Option<&str>, task: &str) -> SubagentRun {
        SubagentRun {
            chat: chat.map(str::to_owned),
            task: Some(task.into()),
        }
    }

    #[test]
    fn the_database_says_which_call_started_a_subagent() {
        let mut chats = Chats::default();
        chats.hear(hook("beforeSubmitPrompt", None, 1_000));
        chats.hear(tool("preToolUse", "c1", Some(call("t1", "Audit")), 1_100));
        chats.hear(tool("preToolUse", "c1", Some(call("t2", "Review")), 1_200));
        // The second call's subagent is heard first.
        chats.hear(tool("preToolUse", "sub2", None, 1_300));
        chats.hear(tool("preToolUse", "sub1", None, 1_400));
        let spawned = |id: &str, call: &str| {
            let mut row = row(id);
            row.spawn = Some(Spawn {
                parent: "c1".into(),
                call: call.into(),
            });
            row
        };
        chats.set_rows(vec![spawned("sub1", "t1"), spawned("sub2", "t2")]);
        let shown = live(&mut chats, 2_000);
        assert_eq!(shown.len(), 1, "no cards of their own");
        assert_eq!(
            shown[0].subagents,
            [run(Some("sub1"), "Audit"), run(Some("sub2"), "Review")]
        );
        // A call none is heard of for yet has none.
        chats.hear(tool("preToolUse", "c1", Some(call("t3", "Plan")), 2_100));
        let shown = live(&mut chats, 2_200);
        assert_eq!(shown[0].subagents[2], run(None, "Plan"));
        // One the database knows is its subagent whatever its hooks say.
        chats.hear(tool("preToolUse", "c1", Some(call("t4", "Test")), 2_300));
        let rows = [("sub1", "t1"), ("sub2", "t2"), ("sub4", "t4")];
        chats.set_rows(rows.map(|(id, call)| spawned(id, call)).to_vec());
        let mut first = tool("preToolUse", "sub4", None, 2_400);
        first.transcript = true;
        chats.hear(first);
        let shown = live(&mut chats, 2_500);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].subagents[3], run(Some("sub4"), "Test"));
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
        assert_eq!(shown[0].subagents, [run(Some("sub1"), "Watch")]);
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
                worktrees: Vec::new(),
                fallback: false,
            },
            Spot {
                dir: Path::new("/home/u/app"),
                worktrees: Vec::new(),
                fallback: false,
            },
            Spot {
                dir: Path::new("/home/u/app/api"),
                worktrees: vec![Path::new("/home/u/app/api-perf")],
                fallback: false,
            },
            Spot {
                dir: Path::new("/home/u/app"),
                worktrees: Vec::new(),
                fallback: true,
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
        // Another worktree of a workspace's repository goes by that
        // workspace, though a workspace's folder holds it less deep.
        assert_eq!(at(&["/home/u/app/api-perf/src"]), Some(2));
        // Not the home folder's workspace: another project's chat.
        assert_eq!(at(&["/home/u/other"]), None);
        // In the project but under no workspace: the Cursor one.
        let lone = [Spot {
            dir: Path::new("/home/u/app"),
            worktrees: Vec::new(),
            fallback: true,
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
            {"command":"'/old/gerdr' cursor-hook"},{"command":"'/new/gerdr (deleted)' cursor-hook"}],
            "afterFileEdit":[{"command":"fmt"}]},"x":2}"#;
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
    fn only_the_cli_in_a_terminal_is_told_from_cursors_chats() {
        let args = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let versions = "/home/u/.local/share/cursor-agent/versions/2026.10.14-941492e";
        let cli = args(&format!(
            "/home/u/.local/bin/cursor-agent --use-system-ca {versions}/index.js"
        ));
        assert!(is_cli_agent(&cli));
        let alias = args(&format!("agent {versions}/index.js --model sonnet"));
        assert!(is_cli_agent(&alias));
        let worker = args(
            "/home/u/p/agent-cli/.local/bin/cursor-agent --use-system-ca \
             /home/u/p/agent-cli/.local/share/cursor-agent/versions/2026.10.22-27dcf5f/index.js \
             --endpoint https://api2.cursor.sh worker start --worker-dir /home/u/w",
        );
        assert!(!is_cli_agent(&worker));
        assert!(!is_cli_agent(&args(
            "/usr/share/cursor/cursor --type=utility"
        )));
        assert!(!is_cli_agent(&args("sh -c gerdr cursor-hook")));
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
        // A process starting on another thread holds a copy until it has started.
        let started = std::time::Instant::now();
        while sender.send_to(&[], &dead).is_ok() {
            assert!(started.elapsed().as_secs() < 5, "still takes datagrams");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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
        // A subagent's, started lately, with the call that started it.
        let subagent = |id: &str, created: i64| {
            conn.execute(
                "INSERT INTO composerHeaders (composerId, createdAt, isSubagent, value)
                 VALUES (?1, ?2, 1, ?3)",
                rusqlite::params![
                    id,
                    created,
                    r#"{"subagentInfo":{"parentComposerId":"a","toolCallId":"t1\nfc_1"}}"#
                ],
            )
            .unwrap();
        };
        subagent("sub", now - 10);
        subagent("sub-old", now - 2 * RECENT_MS);
        let rows = read_rows(&conn, now, &[], &mut known).unwrap();
        let spawned: Vec<_> = rows.iter().filter(|r| r.spawn.is_some()).collect();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].id, "sub");
        assert_eq!(
            spawned[0].spawn,
            Some(Spawn {
                parent: "a".into(),
                call: "t1\nfc_1".into(),
            })
        );
        assert_eq!(spawned[0].status, None);
        // An odd row skips its own chat only, an odd status is none.
        header("odd", now - 10, 0, r#"{"name":5}"#);
        conn.execute(
            "INSERT INTO cursorDiskKV VALUES ('composerData:unread', '{\"status\":7}')",
            [],
        )
        .unwrap();
        known.clear();
        let rows = read_rows(&conn, now, &[], &mut known).unwrap();
        assert!(rows.iter().all(|r| r.id != "odd"));
        assert!(rows.iter().any(|r| r.id == "a"));
        let unread = rows.iter().find(|r| r.id == "unread").unwrap();
        assert_eq!(unread.status, None);
    }

    #[test]
    fn a_hook_time_ahead_of_now_is_now() {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        receiver.set_nonblocking(true).unwrap();
        for at_ms in [i64::MAX, i64::MIN, 5] {
            let message = serde_json::to_vec(&hook("stop", None, at_ms)).unwrap();
            sender.send(&message).unwrap();
        }
        let at: Vec<i64> = take_events(&receiver).iter().map(|e| e.at_ms).collect();
        assert_eq!(at.len(), 3);
        assert!(at[0] <= now_ms() && at[0] > 0);
        assert_eq!(at[1..], [0, 5]);
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

    #[test]
    fn a_done_chat_deleted_is_hidden_until_it_changes() {
        let mut history = History::default();
        let mut news = moded("ask", "chat", 1_000);
        news.unread = true;
        let done = CursorChat {
            status: AgentStatus::Done,
            ..idle(news.clone())
        };
        assert!(!history.hides(&done, None));
        history.delete(&news);
        assert!(history.hides(&done, None));
        // Not while a hook tells of a run ended since, nor once working.
        let ended = hook("stop", Some("completed"), 2_000);
        assert!(!history.hides(&done, Some(&ended)));
        let working = CursorChat {
            status: AgentStatus::Working,
            ..done.clone()
        };
        assert!(!history.hides(&working, None));
        // A change brings it back.
        let changed = CursorChat {
            row: ChatRow {
                updated_ms: 3_000,
                ..news
            },
            ..done
        };
        assert!(!history.hides(&changed, None));
    }

    #[test]
    fn a_chat_closed_while_running_is_deleted_once_its_run_is_over() {
        let mut history = History::default();
        let row = moded("c1", "agent", 1_000);
        let working = CursorChat {
            status: AgentStatus::Working,
            ..idle(row.clone())
        };
        history.close(&row);
        assert!(history.hides(&working, None));
        assert!(!history.settle(std::slice::from_ref(&working), &HashMap::new()));
        // Over by the hooks, but the database doesn't know yet: still hidden.
        let done = CursorChat {
            status: AgentStatus::Done,
            ..working.clone()
        };
        let hooks = HashMap::from([("c1".to_owned(), hook("stop", Some("completed"), 2_000))]);
        assert!(!history.settle(std::slice::from_ref(&done), &hooks));
        assert!(history.hides(&done, hooks.get("c1")));
        // Once it does, deleted as the run left it: back when it changes.
        let written = CursorChat {
            row: ChatRow {
                updated_ms: 2_500,
                ..row
            },
            ..done
        };
        assert!(history.settle(std::slice::from_ref(&written), &hooks));
        assert!(history.closed.is_empty());
        assert!(history.hides(&written, hooks.get("c1")));
        let again = CursorChat {
            status: AgentStatus::Working,
            ..written
        };
        assert!(!history.hides(&again, None));
    }

    #[test]
    fn closing_the_socket_removes_its_file_once() {
        let dir = std::env::temp_dir().join(format!("gerdr-cursor-sock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hooks.sock");
        std::fs::write(&path, "").unwrap();
        let mut watch = crate::state::CursorWatch {
            socket: Some(path.clone()),
            ..Default::default()
        };
        watch.close_socket();
        assert!(!path.exists());
        assert_eq!(watch.socket, None);
        watch.close_socket();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reading_the_database_asks_with_the_history_kept() {
        let (tx, rx) = std::sync::mpsc::channel();
        let watch = crate::state::CursorWatch {
            poke: Some(tx),
            ..Default::default()
        };
        let projects = crate::projects::Store::default();
        watch.read_db(&projects);
        assert_eq!(rx.try_recv(), Ok(projects.cursor_history.ids()));
        // Without a reader nothing is asked.
        crate::state::CursorWatch::default().read_db(&projects);
    }
}
