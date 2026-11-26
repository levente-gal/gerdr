//! An agent's pane shown as a chat (see `/home/lgal/reports/gerdr-agent-chat-plan.md`):
//! the agent keeps running in its herdr pane as it is; gerdr reads its
//! chat's transcript (`source`, `tail`, `parse`) and paints it in the
//! pane's place (`chrome::agent_chat`), and what the user types goes to the
//! pane as if typed there (`view`).

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use crate::{ai_work::AgentKind, md_blocks::DocBlock, text_field::TextField};

pub mod btw;
pub mod commands;
pub mod files;
pub mod images;
pub mod menu;
pub mod mode;
pub mod model;
pub mod parse;
pub mod rows;
pub mod source;
pub mod live;
pub mod questions;
pub mod tail;
mod view;

#[cfg(test)]
pub(crate) use view::{footer, slash_footer};
pub(crate) use view::{suggestion, suggests};

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
    /// A plan the agent wrote (Claude's `ExitPlanMode`, or a file in its
    /// plans folder): its markdown, shown in a card.
    Plan {
        id: String,
        /// Its file, when it was written to one.
        path: Option<String>,
        text: String,
        blocks: Arc<Vec<DocBlock>>,
        output: Option<ToolOut>,
    },
    /// A subagent's work, folded under one card in the place of the call
    /// that started it.
    Subagent {
        id: String,
        title: String,
        items: Arc<Vec<Item>>,
        /// That call's result, once it came.
        output: Option<ToolOut>,
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
            | Item::Plan { id, .. }
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
    /// Its transcript holds more before what was read.
    pub earlier: bool,
    /// The earlier messages were asked for and are being read.
    pub reading_earlier: bool,
    pub focus: ChatFocus,
    /// The reply being written: kept unsaved, as a draft (K2).
    pub draft: TextField,
    /// The `/` menu over the reply box, while the reply is a command being
    /// typed.
    pub slash: Option<commands::Slash>,
    /// The files attached to the reply: their chips show over the reply
    /// box, and their paths go after its words (`files::reply`).
    pub attached: Vec<std::path::PathBuf>,
    /// The pane's last screen rows while its agent waits on the user: the
    /// approval menu, which the transcript doesn't hold. As
    /// `crate::scrollback::parse_ansi` reads them.
    pub waiting: Option<Vec<(String, crate::scrollback::Styled)>>,
    /// What Claude Code suggests the user says next, read off its screen
    /// once its turn ends (`view::suggestion`): the empty reply box shows
    /// it, and Left fills it in.
    pub suggestion: Option<String>,
    /// How often its screen was read for a suggestion since its agent
    /// last worked: it stops after `view::SUGGEST_READS`.
    pub suggest_reads: u8,
    /// What its agent said live: the tools running, an ask (`live`).
    pub live: live::PaneLive,
    /// The user's picks on the card of its ask's questions, till Submit.
    pub card: questions::Card,
    /// The model its agent is on, as its chat last said or as last picked
    /// here (`model`).
    pub model: Option<String>,
    /// How full its agent's context is, as its chat last said.
    pub context: Option<crate::ai_work::context::Context>,
}

/// When an agent's turn began, as herdr's rounds saw its status.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Turn {
    /// When a round first saw it working, in Unix ms; none while it isn't.
    seen: Option<u64>,
    /// When a round last saw it not working, in Unix ms; 0 before any did.
    idle_at: u64,
}

impl Turn {
    /// A round saw its agent at `status` at `now` (Unix ms). Only idle or
    /// done ends a turn: paused (unknown) or asking (blocked), it goes on.
    pub fn note(&mut self, status: crate::herdr::AgentStatus, now: u64) {
        use crate::herdr::AgentStatus as S;
        match status {
            S::Working => _ = self.seen.get_or_insert(now),
            S::Idle | S::Done => (self.seen, self.idle_at) = (None, now),
            S::Blocked | S::Unknown => {}
        }
    }

    /// When the turn began while its agent works: the user's last words
    /// in `items` if they came after it was last seen idle (or gerdr
    /// started mid-turn), else when it was first seen working.
    pub fn since(&self, items: &[Item]) -> Option<u64> {
        let seen = self.seen?;
        let said = items.iter().rev().find_map(|item| match item {
            Item::User { at, .. } => Some(*at),
            _ => None,
        });
        Some(match said.flatten() {
            Some(at) if at >= self.idle_at => at,
            _ => seen,
        })
    }
}

/// What `tail` sends back.
#[derive(Debug)]
pub enum Event {
    /// The chat of the pane, all its items so far.
    Items {
        pane_id: String,
        source: Source,
        items: Arc<Vec<Item>>,
        /// The transcript holds more before them, read on asking
        /// (`Tail::earlier`).
        earlier: bool,
        /// The model the agent is on, where its chat or config says
        /// (`model`).
        model: Option<String>,
        /// How full its context is, where its chat says.
        context: Option<crate::ai_work::context::Context>,
    },
    /// No chat found for the pane, and why.
    Missing { pane_id: String, why: String },
    /// Its transcript couldn't be read.
    Failed { pane_id: String, error: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(at: u64) -> Item {
        Item::User {
            id: "u".into(),
            at: Some(at),
            text: "go".into(),
        }
    }

    #[test]
    fn a_turn_begins_at_the_users_words_or_when_seen_working() {
        use crate::herdr::AgentStatus::{Blocked, Idle, Unknown, Working};
        let mut turn = Turn::default();
        assert_eq!(turn.since(&[user(5)]), None);
        // gerdr started mid-turn: the transcript knows when it began.
        turn.note(Working, 900);
        assert_eq!(turn.since(&[user(5)]), Some(5));
        assert_eq!(turn.since(&[]), Some(900));
        // Later rounds keep the first sighting; paused or asking, it goes on.
        turn.note(Working, 950);
        turn.note(Unknown, 960);
        turn.note(Blocked, 970);
        turn.note(Working, 980);
        assert_eq!(turn.since(&[]), Some(900));
        turn.note(Idle, 1000);
        assert_eq!(turn.since(&[user(5)]), None);
        // Words sent after it went idle start the next turn.
        turn.note(Working, 1300);
        assert_eq!(turn.since(&[user(5), user(1200)]), Some(1200));
        // A turn no words started (old words only): from when it was seen.
        assert_eq!(turn.since(&[user(5)]), Some(1300));
    }
}
