//! What a chat cost, from its transcript: messages, tool calls and tokens,
//! by model. Claude says a message's usage on each of its lines, so they
//! count once by `message.id`; Codex's turn ends with its count; Cursor
//! says no tokens.

use std::collections::BTreeMap;

use super::AgentKind;
use crate::feature::Stage;

/// What one message, or many, cost.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub messages: u64,
    pub tool_calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_create: u64,
    pub cache_read: u64,
}

/// One message of a transcript.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// When, in milliseconds since the epoch.
    pub at_ms: u64,
    pub model: String,
    /// It was a subagent's.
    pub sidechain: bool,
    pub usage: Usage,
}

/// The messages of transcript `raw` of `agent`.
pub fn read(agent: AgentKind, raw: &[u8]) -> Vec<Message> {
    let _ = (agent, raw);
    Vec::new()
}

/// `messages` by run and model: each goes to the latest of `runs` (stage,
/// n, when it began in ms) that began before it.
pub fn split(
    messages: &[Message],
    runs: &[(Stage, u32, u64)],
) -> BTreeMap<(Stage, u32, String), Usage> {
    let _ = (messages, runs);
    BTreeMap::new()
}
