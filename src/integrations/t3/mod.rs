//! T3 Code. Its threads are rows of its database (`userdata/statev2.sqlite`),
//! found with the chat of the agent each ran; gerdr shows no chat of its
//! own for it, and hears no live word from it.

mod find;

use std::collections::HashSet;

use super::Integration;
use crate::ai_work::{
    AgentKind, Chat,
    locate::{Found, Homes, Span},
};

pub struct T3;

impl Integration for T3 {
    fn kind(&self) -> AgentKind {
        AgentKind::T3
    }

    fn label(&self) -> &'static str {
        "T3 Code"
    }

    fn find(
        &self,
        homes: &Homes,
        chat: &Chat,
        id: Option<&str>,
        _claimed: &HashSet<String>,
        _span: Span,
    ) -> Result<Option<Found>, String> {
        match id {
            Some(id) => find::thread(homes, chat, id),
            None => Ok(None),
        }
    }
}
