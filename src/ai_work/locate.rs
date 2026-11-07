//! Where each agent CLI keeps a chat: Claude's `projects/*/<id>.jsonl`,
//! Cursor's `chats/*/<id>/store.db`, Codex's `sessions/*/*/*/rollout-*.jsonl`,
//! OpenCode's database and T3 Code's. On another machine they are looked
//! up by a script and pulled into `.staging` first.

use std::{collections::HashSet, path::PathBuf};

use super::{AgentKind, Chat};

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
}

/// `chat` on this machine, if it is still there; ids in `claimed` belong
/// to other runs.
pub fn here(chat: &Chat, claimed: &HashSet<String>) -> Option<Found> {
    let _ = (chat, claimed);
    None
}

/// The script that lists `chat`'s files on another machine, for `here`'s
/// search there.
pub fn script(chat: &Chat) -> Option<String> {
    let _ = chat;
    None
}
