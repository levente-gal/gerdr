//! T3 Code. Its threads are rows of its database (`userdata/statev2.sqlite`),
//! found with the chat of the agent each ran; gerdr shows no chat of its
//! own for it, and hears no live word from it. Its threads start as
//! drafts in its app, not in a terminal (`launch`).

mod find;
pub(crate) mod launch;

use std::collections::HashSet;

use super::{Integration, PickedFor};
use crate::ai_work::{
    AgentKind, Chat,
    locate::{Found, Homes, Span},
};
use crate::stage_models::Offer;

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

    fn command(&self) -> Option<&'static str> {
        Some(crate::cli_agents::T3)
    }

    fn in_app(&self) -> bool {
        true
    }

    /// Its draft is set to the model (`launch::selection`).
    fn picks_model(&self) -> bool {
        true
    }

    fn offers(&self, listed: &[Offer]) -> Vec<Offer> {
        listed.to_vec()
    }

    fn model_hint(&self, of: PickedFor) -> Option<&'static str> {
        (of == PickedFor::Stage)
            .then_some("Model for this stage's T3 Code draft; type one as provider/model…")
    }
}
