//! The model an agent of a chat runs on, and switching it from the chat or
//! the phone: Claude Code's and Cursor's `/model <id>` take it in one typed
//! line, so gerdr types that line. The others open a menu of their own on
//! `/model`, so their chats offer no model.

use serde_json::Value;

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

/// Claude model id `id` as people call it: `claude-opus-5-5` is "Opus 5.5",
/// a date at its end dropped. An id not of that shape stays as it is.
pub fn label(id: &str) -> String {
    let Some(rest) = id.strip_prefix("claude-") else {
        return id.to_owned();
    };
    let mut parts = rest
        .split('-')
        .filter(|p| !(p.len() == 8 && p.bytes().all(|b| b.is_ascii_digit())));
    let Some(name) = parts.next().filter(|n| !n.is_empty()) else {
        return id.to_owned();
    };
    let mut named = name[..1].to_uppercase() + &name[1..];
    let version: Vec<&str> = parts.collect();
    if !version.is_empty() {
        named = format!("{named} {}", version.join("."));
    }
    named
}

/// The model Claude Code's transcript line `line` says the chat is on now:
/// a main answer's model, or what a `/model` said it set. None for any
/// other line.
pub fn of_line(line: &Value) -> Option<String> {
    let msg = &line["message"];
    if line["type"] == "assistant" && line["isSidechain"] != true {
        let model = msg["model"].as_str()?;
        return (!model.is_empty() && model != "<synthetic>").then(|| label(model));
    }
    // "Set model to `Opus 5.5` and saved as…" (2.1.3xx); "Set model to
    // <bold>opus</bold>" before.
    let text = msg["content"].as_str().or(line["content"].as_str())?;
    // A switch turned down says the model kept: "Kept model as `Opus 5.5`".
    let (_, out) = (text.split_once("<local-command-stdout>Set model to "))
        .or_else(|| text.split_once("<local-command-stdout>Kept model as "))?;
    let out = out.split("</local-command-stdout>").next()?;
    let set = match out.strip_prefix('`') {
        Some(quoted) => quoted.split('`').next()?.to_owned(),
        None => crate::feature::verify::strip_escapes(out).trim().to_owned(),
    };
    let set = set.trim();
    let mut chars = set.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// The model the Claude Code transcript ending in `tail` is on, by its last
/// line that says.
pub fn last_in(tail: &[u8]) -> Option<String> {
    (tail.split(|b| *b == b'\n').rev())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find_map(|line| of_line(&line))
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
    use serde_json::json;

    use super::*;

    #[test]
    fn claude_ids_read_as_their_names() {
        assert_eq!(label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(label("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(label("claude-fable-5-2"), "Fable 5.2");
        assert_eq!(label("gpt-5.2"), "gpt-5.2");
    }

    #[test]
    fn a_transcript_says_its_model_by_its_last_answer_or_model_command() {
        let answer = |model: &str, side: bool| json!({ "type": "assistant", "isSidechain": side, "message": { "model": model, "content": [] } });
        let set = |out: &str| json!({ "type": "user", "message": { "content": format!("<local-command-stdout>Set model to {out}</local-command-stdout>") } });
        let tail = |lines: &[Value]| {
            (lines.iter())
                .map(|l| l.to_string() + "\n")
                .collect::<String>()
                .into_bytes()
        };
        let plain = [
            answer("claude-opus-5-5", false),
            answer("claude-haiku-5-5", true),
        ];
        assert_eq!(last_in(&tail(&plain)).as_deref(), Some("Opus 5.5"));
        let synthetic = [
            answer("claude-opus-5-5", false),
            answer("<synthetic>", false),
        ];
        assert_eq!(last_in(&tail(&synthetic)).as_deref(), Some("Opus 5.5"));
        let now = [
            answer("claude-opus-5-5", false),
            set("`Sonnet 5.5` and saved as your default for new sessions"),
        ];
        assert_eq!(last_in(&tail(&now)).as_deref(), Some("Sonnet 5.5"));
        let kept = [
            set("`Sonnet 5.5` and saved as your default for new sessions"),
            json!({ "type": "user", "message": { "content": "<local-command-stdout>Kept model as `Opus 5.5`</local-command-stdout>" } }),
        ];
        assert_eq!(last_in(&tail(&kept)).as_deref(), Some("Opus 5.5"));
        let older = [set("\u{1b}[1mopus\u{1b}[22m")];
        assert_eq!(last_in(&tail(&older)).as_deref(), Some("Opus"));
        assert_eq!(last_in(b"{\"type\":\"user\"}\n"), None);
    }

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
