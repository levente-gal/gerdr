//! An agent's pane shown as a chat (see `/home/lgal/reports/gerdr-agent-chat-plan.md`):
//! the agent keeps running in its herdr pane as it is; gerdr reads its
//! chat's transcript (`source`, `tail`, `parse`) and paints it in the
//! pane's place (`chrome::agent_chat`), and what the user types goes to the
//! pane as if typed there (`view`).

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use crate::{ai_work::AgentKind, md_blocks::DocBlock, text_field::TextField};

pub mod parse;
pub mod rows;
pub mod source;
pub mod tail;
mod view;

#[cfg(test)]
pub(crate) use view::footer;

/// One thing said or done in a chat, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// What the user typed.
    User {
        id: String,
        at: Option<u64>,
        text: String,
    },
    /// The agent's answer, as markdown blocks.
    Assistant {
        id: String,
        at: Option<u64>,
        blocks: Arc<Vec<DocBlock>>,
    },
    /// The agent's thinking, shown folded.
    Thinking { id: String, text: String },
    /// A tool call, with its result once it came.
    Tool {
        id: String,
        name: String,
        /// The one line it says: a command, a path, a pattern.
        gist: String,
        input: String,
        output: Option<ToolOut>,
    },
    /// A file edited or written, with its change.
    Edit {
        id: String,
        path: String,
        diff: Vec<DiffLine>,
        output: Option<ToolOut>,
    },
    /// A subagent's work, folded under one card.
    Subagent {
        id: String,
        title: String,
        items: Arc<Vec<Item>>,
    },
    /// What gerdr says in the chat's place: compaction, an interruption, a
    /// line it couldn't read (naming its type).
    Note { id: String, text: String },
}

impl Item {
    /// Its id: unique in its chat, kept as the chat grows (folds go by it).
    pub fn id(&self) -> &str {
        match self {
            Item::User { id, .. }
            | Item::Assistant { id, .. }
            | Item::Thinking { id, .. }
            | Item::Tool { id, .. }
            | Item::Edit { id, .. }
            | Item::Subagent { id, .. }
            | Item::Note { id, .. } => id,
        }
    }
}

/// A tool's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOut {
    pub text: String,
    pub failed: bool,
    /// How long it took, when the transcript says.
    pub ms: Option<u64>,
}

/// A line of an edit's change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub side: DiffSide,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSide {
    Context,
    Added,
    Removed,
}

/// Where a pane's chat is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub agent: AgentKind,
    /// The chat's id.
    pub id: String,
    /// Its transcript: a file of lines (Claude, Codex), or the database
    /// that holds it (OpenCode's `opencode.db`, a Cursor chat's `store.db`).
    pub path: PathBuf,
    /// Found by the agent's folder and start time, as herdr named no chat
    /// for the pane: it may be another chat of that agent there, and the
    /// chat's head says so.
    pub guessed: bool,
}

/// How far finding and reading a pane's chat got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Reading {
    /// Looking for it, or reading it the first time.
    #[default]
    Finding,
    /// Read: the items are its.
    Read,
    /// No chat found, and why, said to the user.
    Missing(String),
    /// Found, but it couldn't be read, and why.
    Failed(String),
}

/// What has the chat's keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatFocus {
    /// The items: move, fold, copy.
    #[default]
    List,
    /// The reply box.
    Composer,
}

/// A pane shown as a chat, from the first time it was asked for: kept while
/// its screen shows again, so flipping back is instant.
#[derive(Default)]
pub struct PaneChat {
    /// Chat (true) or the pane's own screen (false).
    pub shown: bool,
    pub source: Option<Source>,
    pub reading: Reading,
    pub items: Arc<Vec<Item>>,
    /// The ids of items unfolded, and of those whose output shows whole
    /// (`chrome::whole_key`).
    pub unfolded: HashSet<String>,
    /// The item picked with the keys, by index.
    pub picked: Option<usize>,
    /// How far down the list is scrolled, in logical pixels.
    pub top: f64,
    /// The list follows new items while it is at the bottom.
    pub stick: bool,
    pub focus: ChatFocus,
    /// The reply being written: kept unsaved, as a draft (K2).
    pub draft: TextField,
    /// The pane's last screen rows while its agent waits on the user: the
    /// approval menu, which the transcript doesn't hold. As
    /// `crate::scrollback::parse_ansi` reads them.
    pub waiting: Option<Vec<(String, crate::scrollback::Styled)>>,
}

/// What `tail` sends back.
#[derive(Debug)]
pub enum Event {
    /// The chat of the pane, all its items so far.
    Items {
        pane_id: String,
        source: Source,
        items: Arc<Vec<Item>>,
    },
    /// No chat found for the pane, and why.
    Missing { pane_id: String, why: String },
    /// Its transcript couldn't be read.
    Failed { pane_id: String, error: String },
}
