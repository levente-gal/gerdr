//! A transcript read into chat items, as it grows.

use super::Item;
use crate::ai_work::AgentKind;

/// Reads one chat's transcript, fed its bytes as they come.
pub struct Reader {
    agent: AgentKind,
    items: Vec<Item>,
}

impl Reader {
    pub fn new(agent: AgentKind) -> Self {
        Self {
            agent,
            items: Vec::new(),
        }
    }

    /// Read `lines`, whole lines of the transcript after those fed before
    /// (each ending in `\n`). An item already read can change: a tool call
    /// gets its result.
    pub fn feed(&mut self, lines: &[u8]) {
        let _ = (self.agent, lines);
    }

    /// The items read so far, in order.
    pub fn items(&self) -> &[Item] {
        &self.items
    }
}
