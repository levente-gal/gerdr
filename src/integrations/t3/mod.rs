//! T3 Code: its threads are found, with the chat of the agent each ran;
//! gerdr shows no chat of its own for it.

use std::collections::HashSet;

use super::Integration;
use crate::ai_work::{
    AgentKind, Chat,
    locate::{self, Found, Homes, Span},
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
            Some(id) => locate::t3(homes, chat, id),
            None => Ok(None),
        }
    }
}
