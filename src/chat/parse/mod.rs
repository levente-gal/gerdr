//! A transcript read into chat items, as it grows.
//!
//! Claude Code's (`claude`), Codex's rollout (`codex`), pi's and the
//! Antigravity CLI's are lines, fed as they come. OpenCode's and
//! Cursor's chats are in databases: their rows are read whole each time they
//! change, and `snapshot` makes the items of all of them (`opencode`,
//! `cursor`). Item ids come from the transcript (a message's, a call's), so
//! they stay the same when it is read again and folds keep.

pub(crate) mod antigravity;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod opencode;
pub(crate) mod patch;

pub(crate) use opencode::gist;

use serde_json::Value;

use super::{DiffLine, Item, ToolOut};
#[cfg(test)]
pub(crate) use crate::integrations::common::build::plan_item;
use crate::{
    ai_work::AgentKind,
    integrations::{Parser, Store, common::build::Builder},
};

/// Reads one chat's transcript, fed its bytes as they come: what all
/// agents' readers keep, and what its agent's keeps of its own.
pub struct Reader {
    pub(crate) b: Builder,
    pub(crate) p: Box<dyn Parser>,
    /// Whether its chat is lines, fed; a database chat's rows are read
    /// with `snapshot`.
    lines: bool,
}

impl Reader {
    pub fn new(agent: AgentKind) -> Self {
        let integration = crate::integrations::of(agent);
        Self {
            b: Builder::default(),
            p: integration.parser(),
            lines: integration.store() == Some(Store::Lines),
        }
    }

    /// Read `lines`, whole lines of the transcript after those fed before
    /// (each ending in `\n`). An item already read can change: a tool call
    /// gets its result.
    pub fn feed(&mut self, lines: &[u8]) {
        if !self.lines {
            return;
        }
        let b = &mut self.b;
        b.rest.extend_from_slice(lines);
        let Some(end) = b.rest.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let whole: Vec<u8> = b.rest.drain(..=end).collect();
        for raw in whole[..end].split(|b| *b == b'\n') {
            b.lines += 1;
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<Value>(raw) {
                Ok(line) => self.p.line(b, &line),
                // Reading can start mid-file: the first line may be cut short.
                Err(_) if b.lines == 1 => {}
                Err(_) => {
                    let id = format!("line{}", b.lines);
                    let text = "A line of the transcript gerdr couldn't read".to_owned();
                    b.push(Item::Note { id, text });
                }
            }
        }
        b.sync_subs();
    }

    /// Whole lines of subagent `agent`'s own transcript, apart from the
    /// chat's; `call` is the call that started it, as the agent says.
    pub fn feed_subagent(&mut self, agent: &str, call: &str, lines: &[u8]) {
        self.p.subagent(&mut self.b, agent, call, lines);
        self.b.sync_subs();
    }

    /// Whether tool call `call` was read: a subagent it started has a
    /// place to show.
    pub fn knows_call(&self, call: &str) -> bool {
        self.b.knows_call(call)
    }

    /// The model the chat is on, as its lines read so far last said.
    pub fn model(&self) -> Option<String> {
        self.p.model()
    }

    /// The items read so far, in order.
    pub fn items(&self) -> &[Item] {
        self.b.items()
    }

    /// The conversations results named as subagents not taken yet.
    pub fn named_subagents(&self) -> Vec<String> {
        self.p.named_subagents(&self.b)
    }

    /// How many subagents asked for have no conversation yet.
    pub fn unfound_subagents(&self) -> usize {
        self.p.unfound_subagents()
    }

    /// The conversations taken as subagents.
    pub fn subagents(&self) -> Vec<String> {
        self.p.subagents()
    }

    /// Take conversation `child` as a subagent, its transcript's first
    /// line `first`; whether it was taken.
    pub fn take_subagent(&mut self, child: &str, first: &str) -> bool {
        self.p.take_subagent(&mut self.b, child, first)
    }
}

/// The items of a chat kept in a database, from all its rows in order:
/// OpenCode's message rows as `ai_work::locate` reads them (a `message`
/// row with its `parts`, or a `session_message` row, its JSON columns as
/// what they hold), Cursor's `store.db` messages as
/// `ai_work::usage::cursor_messages` gives them. Other agents' have none.
pub fn snapshot<'a>(agent: AgentKind, rows: impl IntoIterator<Item = &'a Value>) -> Vec<Item> {
    let mut reader = Reader::new(agent);
    reader.p.rows(&mut reader.b, &mut rows.into_iter());
    reader.b.items
}

#[cfg(test)]
mod tests {
    use crate::integrations::common::build::{MAX_BYTES, MAX_LINES, cap};

    #[test]
    fn long_output_is_capped() {
        let long: String = (0..MAX_LINES + 2).map(|n| format!("{n}\n")).collect();
        let kept = cap(&long);
        assert_eq!(kept.lines().count(), MAX_LINES + 1);
        assert!(
            kept.ends_with("\u{2026} 2 more lines"),
            "{}",
            &kept[kept.len() - 40..]
        );
        let wide = "é".repeat(MAX_BYTES);
        assert!(cap(&wide).len() <= MAX_BYTES + 40);
        assert_eq!(cap("short\n"), "short\n");
    }
}
