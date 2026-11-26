//! What a pi chat says of itself: how full its context is, what its
//! messages cost.

use crate::ai_work::{
    context::{Context, lines_back, num},
    usage::{Message, Usage, json_lines},
};

/// The context of the pi chat whose transcript ends with `tail`: each assistant message says what its
/// request sent; pi names no window.
pub(crate) fn context(tail: &[u8]) -> Option<Context> {
    lines_back(tail).find_map(|v| {
        let m = &v["message"];
        if v["type"] != "message" || m["role"] != "assistant" {
            return None;
        }
        let u = &m["usage"];
        let used = num(&u["input"]) + num(&u["cacheRead"]) + num(&u["cacheWrite"]);
        (used > 0).then_some(Context { used, window: None })
    })
}

/// pi's session: each assistant message says what its request used, its
/// cached input apart.
pub(crate) fn usage(raw: &[u8]) -> Vec<Message> {
    (json_lines(raw))
        .filter_map(|(_, line)| {
            let m = &line["message"];
            if line["type"] != "message" || m["role"] != "assistant" {
                return None;
            }
            let u = &m["usage"];
            let parts = m["content"].as_array().into_iter().flatten();
            Some(Message {
                at_ms: num(&m["timestamp"]),
                model: m["model"].as_str().unwrap_or_default().to_owned(),
                sidechain: false,
                usage: Usage {
                    messages: 1,
                    tool_calls: parts.filter(|p| p["type"] == "toolCall").count() as u64,
                    input: num(&u["input"]),
                    output: num(&u["output"]),
                    cache_create: num(&u["cacheWrite"]),
                    cache_read: num(&u["cacheRead"]),
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::ai_work::{AgentKind, usage::read};

    #[test]
    fn pi_messages_say_their_own_usage() {
        let raw = concat!(
            r#"{"type":"session","id":"s"}"#,
            "\n",
            r#"{"type":"message","message":{"role":"user","content":"hi","timestamp":1}}"#,
            "\n",
            r#"{"type":"message","message":{"role":"assistant","model":"m1","timestamp":2,"#,
            r#""content":[{"type":"toolCall"},{"type":"text"}],"#,
            r#""usage":{"input":10,"output":3,"cacheRead":100,"cacheWrite":7}}}"#,
            "\n",
        );
        let got = read(AgentKind::Pi, raw.as_bytes());
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].at_ms, got[0].model.as_str()), (2, "m1"));
        let u = &got[0].usage;
        assert_eq!((u.messages, u.tool_calls, u.input, u.output), (1, 1, 10, 3));
        assert_eq!((u.cache_create, u.cache_read), (7, 100));
    }
}
