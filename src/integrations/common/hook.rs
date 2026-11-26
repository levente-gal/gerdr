//! What every agent's live hook does alike (see `chat::live`): read the
//! hook's input from stdin and tell the gerdr of the pane it runs in.

use std::io::Read;

use serde_json::Value;

use crate::chat::live::{self, Answer, LiveEvent, What};

/// The most of a hook's input read.
pub const MAX_INPUT: u64 = 4 << 20;

/// The hook's input on stdin, as JSON; `None` when it isn't.
pub fn input() -> Option<Value> {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
    serde_json::from_slice(&input).ok()
}

/// Tells the gerdr of this hook's pane (by `HERDR_SOCKET_PATH` and
/// `HERDR_PANE_ID`) that `agent` did `what` at `at_ms`. The chat's answer,
/// for an ask; `None` outside a herdr pane or when no gerdr listens.
pub fn tell(agent: &str, at_ms: i64, what: What) -> Option<Answer> {
    let (socket, pane) = live::from_env()?;
    let event = LiveEvent {
        pane,
        agent: agent.into(),
        at_ms,
        what,
    };
    live::send(&socket, &event)
}
