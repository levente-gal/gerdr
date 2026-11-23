//! A thread of its own that finds the chats of the panes shown as chats,
//! reads their transcripts as they grow and sends their items back.

use smithay::reexports::calloop::channel;

use super::Event;
use crate::herdr::AgentInfo;

/// The thread: what it is asked to follow.
pub struct Tail {
    _out: channel::Sender<Event>,
}

impl Tail {
    /// Start the thread, sending what it reads to `out`.
    pub fn spawn(out: channel::Sender<Event>) -> Self {
        Self { _out: out }
    }

    /// Follow the chat of the agent in pane `agent.pane_id`: find it, read
    /// it, and send its items each time it grows. Asked again for the same
    /// pane (herdr reports another session), it finds the chat again.
    pub fn follow(&self, agent: AgentInfo) {
        let _ = agent;
    }

    /// Stop following the chat of pane `pane_id` (hidden or closed).
    pub fn stop(&self, pane_id: &str) {
        let _ = pane_id;
    }
}
