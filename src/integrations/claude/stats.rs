//! What a Claude Code chat says of itself: how full its context is, what
//! its messages cost.

use std::collections::{HashMap, HashSet};

use super::parse::main_model;
use crate::{
    ai_work::{
        context::{Context, num},
        usage::{Message, Usage, iso_ms},
    },
    integrations::common::files::{json_lines, lines_back},
};

/// The windows of Claude's models before the 4.6 family; all since take 1M.
const SMALL_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;

/// The window of Claude model `model`: 200k for Haiku 4.5 and the models
/// before Opus and Sonnet 4.6, 1M for every one since (and any not known
/// yet, as new ones are likely to keep it).
fn claude_window(model: &str) -> u64 {
    let old = [
        "-4-5", "-4-1", "-4-0", "-4-2025", "-3-", "claude-3", "claude-2",
    ];
    match old.iter().any(|o| model.contains(o)) {
        true => SMALL_WINDOW,
        false => LARGE_WINDOW,
    }
}

/// The context of the Claude Code chat whose transcript ends with `tail`:
/// what its last main answer sent.
pub(crate) fn context(tail: &[u8]) -> Option<Context> {
    lines_back(tail).find_map(|v| {
        let model = main_model(&v)?;
        let u = &v["message"]["usage"];
        let used = num(&u["input_tokens"])
            + num(&u["cache_creation_input_tokens"])
            + num(&u["cache_read_input_tokens"]);
        // More than the window the name gives: it runs on the large one.
        let window = Some(claude_window(model)).map(|w| match used > w {
            true => LARGE_WINDOW.max(w),
            false => w,
        });
        (used > 0).then_some(Context { used, window })
    })
}

/// Claude Code's transcript: a line per content block of an assistant
/// message, each repeating the message's usage.
pub(crate) fn usage(raw: &[u8]) -> Vec<Message> {
    struct Seen {
        message: Message,
        tools: HashSet<String>,
        unnamed_tools: u64,
    }
    let mut order = Vec::new();
    let mut seen: HashMap<String, Seen> = HashMap::new();
    for (i, line) in json_lines(raw) {
        if line["type"] != "assistant" {
            continue;
        }
        let msg = &line["message"];
        let model = msg["model"].as_str().unwrap_or_default();
        if model == "<synthetic>" {
            continue;
        }
        let key = (msg["id"].as_str().or(line["requestId"].as_str()))
            .map_or_else(|| format!("line {i}"), str::to_owned);
        let entry = seen.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            Seen {
                message: Message {
                    at_ms: line["timestamp"].as_str().and_then(iso_ms).unwrap_or(0),
                    model: model.to_owned(),
                    sidechain: line["isSidechain"].as_bool().unwrap_or(false),
                    usage: Usage {
                        messages: 1,
                        ..Usage::default()
                    },
                },
                tools: HashSet::new(),
                unnamed_tools: 0,
            }
        });
        // The same on every line of a message, but for output that may grow
        // as it streams: the most said counts.
        let usage = &msg["usage"];
        let u = &mut entry.message.usage;
        u.input = u.input.max(num(&usage["input_tokens"]));
        u.output = u.output.max(num(&usage["output_tokens"]));
        u.cache_create = (u.cache_create).max(num(&usage["cache_creation_input_tokens"]));
        u.cache_read = u.cache_read.max(num(&usage["cache_read_input_tokens"]));
        let blocks = msg["content"].as_array().into_iter().flatten();
        for block in blocks.filter(|b| b["type"] == "tool_use") {
            match block["id"].as_str() {
                Some(id) => {
                    entry.tools.insert(id.to_owned());
                }
                None => entry.unnamed_tools += 1,
            }
        }
    }
    (order.into_iter())
        .filter_map(|key| seen.remove(&key))
        .map(|mut s| {
            s.message.usage.tool_calls = s.tools.len() as u64 + s.unnamed_tools;
            s.message
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::{AgentKind, context::context, usage::read};

    fn claude(model: &str, sidechain: bool, input: u64, created: u64, read: u64) -> String {
        format!(
            "{{\"type\":\"assistant\",\"isSidechain\":{sidechain},\"message\":{{\"model\":\"{model}\",\"usage\":{{\"input_tokens\":{input},\"cache_creation_input_tokens\":{created},\"cache_read_input_tokens\":{read},\"output_tokens\":9}}}}}}\n"
        )
    }

    #[test]
    fn a_claude_chat_counts_what_its_last_main_answer_sent() {
        // A line cut where the tail began, then the chat as it ends.
        let tail = [
            "\"usage\":{\"input_tokens\":3}}\n".to_owned(),
            claude("claude-opus-5-5", false, 10, 0, 1_000),
            claude("claude-opus-5-5", false, 2, 500, 83_500),
            claude("claude-haiku-5-5", true, 7, 0, 7),
            claude("<synthetic>", false, 0, 0, 0),
            "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n".to_owned(),
        ]
        .concat();
        let got = context(AgentKind::Claude, tail.as_bytes());
        assert_eq!(
            got,
            Some(Context {
                used: 84_002,
                window: Some(LARGE_WINDOW)
            })
        );
        assert!(!got.unwrap().nearly_full());
        assert_eq!(context(AgentKind::Claude, b""), None);
    }

    #[test]
    fn claude_windows_go_by_model() {
        assert_eq!(claude_window("claude-opus-5-5"), LARGE_WINDOW);
        assert_eq!(claude_window("claude-sonnet-4-6"), LARGE_WINDOW);
        assert_eq!(claude_window("claude-haiku-4-5"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-haiku-4-5-20251001"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-sonnet-4-20250514"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-3-7-sonnet"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-next-9"), LARGE_WINDOW);
        // Over the small window: it runs on the large one.
        let tail = claude("claude-haiku-4-5", false, 0, 0, 250_000);
        let got = context(AgentKind::Claude, tail.as_bytes()).unwrap();
        assert_eq!(got.window, Some(LARGE_WINDOW));
        let tail = claude("claude-haiku-4-5", false, 0, 0, 170_000);
        assert!(
            context(AgentKind::Claude, tail.as_bytes())
                .unwrap()
                .nearly_full()
        );
    }

    fn claude_line(id: &str, at: &str, tool_id: Option<&str>) -> String {
        let content = match tool_id {
            Some(t) => format!(r#"[{{"type":"tool_use","id":"{t}","name":"Read"}}]"#),
            None => r#"[{"type":"text","text":"hi"}]"#.to_owned(),
        };
        format!(
            r#"{{"type":"assistant","requestId":"req_{id}","timestamp":"{at}","isSidechain":false,"message":{{"id":"msg_{id}","model":"claude-opus-5-5","usage":{{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":50}},"content":{content}}}}}"#
        )
    }

    #[test]
    fn claude_usage_counts_once_per_message() {
        let raw = [
            r#"{"type":"user","message":{"content":"go"}}"#.to_owned(),
            claude_line("a", "2026-11-03T10:28:06.709Z", None),
            claude_line("a", "2026-11-03T10:28:06.718Z", Some("t1")),
            claude_line("a", "2026-11-03T10:28:06.720Z", Some("t2")),
            claude_line("b", "2026-11-03T10:28:10.000Z", Some("t3")),
            r#"{"type":"assistant","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":9}}}"#.to_owned(),
            r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-11-03T10:28:11Z","message":{"model":"claude-haiku","usage":{"output_tokens":7}}}"#.to_owned(),
            "not json".to_owned(),
        ]
        .join("\n");
        let got = read(AgentKind::Claude, raw.as_bytes());
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].at_ms, 1_793_701_686_709);
        assert_eq!(
            got[0].usage,
            Usage {
                messages: 1,
                tool_calls: 2,
                input: 2,
                output: 50,
                cache_create: 100,
                cache_read: 1000,
            }
        );
        assert_eq!(got[1].usage.tool_calls, 1);
        assert!(got[2].sidechain);
        assert_eq!(got[2].model, "claude-haiku");
        assert_eq!(got[2].usage.output, 7);
        let total = got.iter().fold(Usage::default(), |mut t, m| {
            t.add(&m.usage);
            t
        });
        assert_eq!(total.input, 4);
        assert_eq!(total.output, 107);
    }
}
