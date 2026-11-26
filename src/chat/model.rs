//! The model an agent of a chat runs on, and switching it from the chat or
//! the phone: Claude Code's and Cursor's `/model <id>` take it in one typed
//! line, so gerdr types that line. The others open a menu of their own on
//! `/model`, so their chats offer no model.

use crate::{
    ai_work::AgentKind,
    integrations::{ModelSwitch, of},
    stage_models::Offer,
};

/// Whether a chat with agent `kind` offers to switch its model.
pub fn picks(kind: AgentKind) -> bool {
    matches!(of(kind).model_switch(), ModelSwitch::Typed { .. })
}

/// The command `kind`'s CLI runs as, as the stage picker names it (for
/// `Flavor::of` and the read of Cursor's models).
pub fn base(kind: AgentKind) -> &'static str {
    match of(kind).model_switch() {
        ModelSwitch::Typed { cli } => cli,
        _ => "claude",
    }
}

/// What `kind`'s CLI is called, for the picker's rows.
pub fn name(kind: AgentKind) -> &'static str {
    of(kind).label()
}

/// What gerdr types to switch an agent to model `id`.
pub fn line(id: &str) -> String {
    format!("/model {id}")
}

/// What a chat offers for agent `kind`: Claude Code's aliases, `cursor`
/// (Cursor's models, as last read) for Cursor.
pub fn offers(kind: AgentKind, cursor: &[Offer]) -> Vec<Offer> {
    of(kind).offers(cursor)
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
