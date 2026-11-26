//! What a Codex chat says of itself: how full its context is and what it
//! cost, from its rollout's `token_count` events.

use serde_json::Value;

use crate::{
    ai_work::{
        context::{Context, num},
        usage::{Message, Usage, iso_ms},
    },
    integrations::common::files::{json_lines, lines_back},
};

/// Codex's, as `context` reads it.
pub(crate) fn codex_context(tail: &[u8]) -> Option<Context> {
    lines_back(tail).find_map(|v| {
        let p = &v["payload"];
        let counted = v["type"] == "event_msg" && p["type"] == "token_count";
        let info = &p["info"];
        if !counted || !info.is_object() {
            return None;
        }
        // Its input tokens count the cached ones too.
        let used = num(&info["last_token_usage"]["input_tokens"]);
        let window = info["model_context_window"].as_u64();
        (used > 0).then_some(Context { used, window })
    })
}

/// Running totals of a Codex chat, as its `token_count` events say them.
#[derive(Clone, Copy, Default)]
struct Totals {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
}

impl Totals {
    fn of(info: &Value) -> Self {
        let t = &info["total_token_usage"];
        Self {
            input: num(&t["input_tokens"]),
            cached: num(&t["cached_input_tokens"]),
            cache_write: num(&t["cache_write_input_tokens"]),
            output: num(&t["output_tokens"]),
        }
    }
}

/// A Codex turn being read: when it began, its last totals, its tool
/// calls.
#[derive(Default)]
struct Turn {
    at_ms: u64,
    last: Option<Totals>,
    tools: u64,
}

/// Codex's rollout: each turn's `token_count` events hold the chat's
/// running totals, so a turn costs its last total less the turn before's.
/// A turn is one message, at its start.
pub(crate) fn codex(raw: &[u8]) -> Vec<Message> {
    let mut out = Vec::new();
    let mut model = String::new();
    let mut before = Totals::default();
    let mut turn: Option<Turn> = None;
    let close = |turn: Option<Turn>, before: &mut Totals, model: &str, out: &mut Vec<Message>| {
        let Some(Turn {
            at_ms,
            last: Some(last),
            tools,
        }) = turn
        else {
            return;
        };
        let less = |a: u64, b: u64| a.saturating_sub(b);
        let cached = less(last.cached, before.cached);
        out.push(Message {
            at_ms,
            model: model.to_owned(),
            sidechain: false,
            usage: Usage {
                messages: 1,
                tool_calls: tools,
                // Codex counts cached input within its input.
                input: less(less(last.input, before.input), cached),
                output: less(last.output, before.output),
                cache_create: less(last.cache_write, before.cache_write),
                cache_read: cached,
            },
        });
        *before = last;
    };
    for (_, line) in json_lines(raw) {
        let at_ms = line["timestamp"].as_str().and_then(iso_ms).unwrap_or(0);
        let begun = || Turn {
            at_ms,
            ..Turn::default()
        };
        let payload = &line["payload"];
        let kind = payload["type"].as_str().unwrap_or_default();
        match (line["type"].as_str().unwrap_or_default(), kind) {
            ("turn_context", _) => {
                if let Some(m) = payload["model"].as_str() {
                    model = m.to_owned();
                }
            }
            ("event_msg", "task_started") => {
                close(turn.take(), &mut before, &model, &mut out);
                turn = Some(begun());
            }
            ("event_msg", "token_count") if payload["info"].is_object() => {
                turn.get_or_insert_with(begun).last = Some(Totals::of(&payload["info"]));
            }
            ("event_msg", "task_complete") => close(turn.take(), &mut before, &model, &mut out),
            ("response_item", k) if k.ends_with("_call") => {
                turn.get_or_insert_with(begun).tools += 1;
            }
            _ => {}
        }
    }
    close(turn, &mut before, &model, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::{AgentKind, usage::read};

    #[test]
    fn codex_turns_cost_their_last_total_less_the_last_turns() {
        let tc = |at: &str, input: u64, cached: u64, output: u64| {
            format!(
                r#"{{"timestamp":"{at}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output}}}}}}}}}"#
            )
        };
        let raw = [
            r#"{"timestamp":"2026-08-26T11:00:10.570Z","type":"session_meta","payload":{"id":"s"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:00:10.571Z","type":"event_msg","payload":{"type":"task_started"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:00:10.965Z","type":"turn_context","payload":{"model":"gpt-5.6-luna"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:00:15.478Z","type":"response_item","payload":{"type":"custom_tool_call"}}"#.to_owned(),
            tc("2026-08-26T11:00:15.576Z", 100, 40, 10),
            r#"{"timestamp":"2026-08-26T11:00:16Z","type":"response_item","payload":{"type":"function_call"}}"#.to_owned(),
            tc("2026-08-26T11:00:22.306Z", 300, 200, 30),
            r#"{"timestamp":"2026-08-26T11:00:29Z","type":"event_msg","payload":{"type":"task_complete"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:02:34Z","type":"event_msg","payload":{"type":"task_started"}}"#.to_owned(),
            tc("2026-08-26T11:02:51Z", 500, 300, 45),
            r#"{"timestamp":"2026-08-26T11:03:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#.to_owned(),
        ]
        .join("\n");
        let got = read(AgentKind::Codex, raw.as_bytes());
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].model, "gpt-5.6-luna");
        assert_eq!(got[0].at_ms, iso_ms("2026-08-26T11:00:10.571Z").unwrap());
        assert_eq!(
            got[0].usage,
            Usage {
                messages: 1,
                tool_calls: 2,
                input: 100,
                output: 30,
                cache_create: 0,
                cache_read: 200,
            }
        );
        assert_eq!(
            got[1].usage,
            Usage {
                messages: 1,
                tool_calls: 0,
                input: 100,
                output: 15,
                cache_create: 0,
                cache_read: 100,
            }
        );
    }
}
