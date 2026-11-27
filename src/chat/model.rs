//! The model an agent of a chat runs on, and switching it from the chat or
//! the phone: Claude Code's and Cursor's `/model <id>` take it in one typed
//! line, so gerdr types that line. The others open a menu of their own on
//! `/model`, so their chats offer no model.

use crate::{ai_work::AgentKind, integrations::of, stage_models::Offer};

/// The command `kind`'s CLI runs as, as the stage picker names it (for
/// `integrations::by_command` and the read of the models it lists).
pub fn base(kind: AgentKind) -> &'static str {
    of(kind).command().unwrap_or("claude")
}

/// What `kind`'s CLI is called, for the picker's rows.
pub fn name(kind: AgentKind) -> &'static str {
    of(kind).label()
}

/// What gerdr types to switch an agent to model `id`.
pub fn line(id: &str) -> String {
    format!("/model {id}")
}

/// What a chat offers for agent `kind`, of `listed`: the models its CLI
/// listed, as last read.
pub fn offers(kind: AgentKind, listed: &[Offer]) -> Vec<Offer> {
    of(kind).offers(listed)
}

/// Whether the models a chat offers for agent `kind` are read from its CLI.
pub fn read(kind: AgentKind) -> bool {
    of(kind).models_read()
}

/// The command of the CLI whose models are read (`read`), to read them by.
pub fn read_from() -> Option<&'static str> {
    (crate::integrations::ALL.iter())
        .find(|agent| agent.models_read())
        .map(|agent| base(agent.kind()))
}

/// The id among `offers` of the model named `now`: by its id or name, or the
/// alias its name begins with ("Opus 5.5" is `opus`).
pub fn picked(offers: &[Offer], now: &str) -> Option<String> {
    let lower = now.to_lowercase();
    let word = lower.split_whitespace().next().unwrap_or_default();
    (offers.iter())
        .find(|o| o.id.eq_ignore_ascii_case(now) || o.name.eq_ignore_ascii_case(now))
        .or_else(|| offers.iter().find(|o| o.id == word))
        .map(|o| o.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_named_is_picked_among_the_offers() {
        let claude = offers(AgentKind::Claude, &[]);
        assert_eq!(picked(&claude, "Opus 5.5").as_deref(), Some("opus"));
        assert_eq!(picked(&claude, "Haiku").as_deref(), Some("haiku"));
        assert_eq!(picked(&claude, "gpt-5.2"), None);
        let cursor = [Offer {
            id: "gpt-5.2".into(),
            name: "GPT-5.2".into(),
        }];
        assert_eq!(
            picked(&offers(AgentKind::Cursor, &cursor), "GPT-5.2").as_deref(),
            Some("gpt-5.2")
        );
        assert!(offers(AgentKind::Pi, &cursor).is_empty());
        assert_eq!(line("sonnet"), "/model sonnet");
    }
}
