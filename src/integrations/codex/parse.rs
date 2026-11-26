//! Codex's rollout (`sessions/<y>/<m>/<d>/rollout-<time>-<id>.jsonl`): a
//! JSON line per thing said or done, `{timestamp, type, payload}`.
//!
//! The chat is in the `response_item` lines: messages, reasoning, tool calls
//! and their outputs. `event_msg` lines mostly say the same again (they are
//! left out), but some things only they hold: edits and web searches made
//! from inside a script (`exec`), an interrupted turn, an error.
//!
//! shortcut: rollouts from before Codex wrapped its lines in
//! `{timestamp, type, payload}` read as nothing; read bare response items too
//! if such old chats turn up.

use serde_json::Value;

use crate::{
    ai_work::usage::iso_ms,
    chat::{Item, ToolOut, parse::patch},
    integrations::{
        Parser,
        common::build::{
            Builder, CHAT_COMPACTED, INTERRUPTED, cant_show, cap, capped, change, plan_item,
        },
    },
    stats_panel::tool_gist,
};

/// A reader of Codex's: it keeps nothing of its own.
pub(crate) struct Codex;

impl Parser for Codex {
    /// Read rollout line `line`.
    fn line(&mut self, b: &mut Builder, line: &Value) {
        let at = line["timestamp"].as_str().and_then(iso_ms);
        let p = &line["payload"];
        match line["type"].as_str().unwrap_or_default() {
            "response_item" => codex_item(b, p, at),
            "event_msg" => codex_event(b, p),
            "compacted" => {
                let id = codex_id(b, p);
                let text = CHAT_COMPACTED.to_owned();
                b.push(Item::Note { id, text });
            }
            // session_meta, turn_context, world_state, token_usage_record:
            // Codex's bookkeeping.
            _ => {}
        }
    }
}

fn codex_item(b: &mut Builder, p: &Value, at: Option<u64>) {
    let id = codex_id(b, p);
    let call = p["call_id"].as_str().unwrap_or(&id).to_owned();
    let name = p["name"].as_str().unwrap_or("tool");
    match p["type"].as_str().unwrap_or_default() {
        "message" => match p["role"].as_str() {
            Some("user") => {
                let text = typed(&p["content"]);
                if !text.is_empty() {
                    b.push(Item::User { id, at, text });
                }
            }
            Some("assistant") => {
                let text = parts(&p["content"]).concat();
                // Plan mode's plan, in its block: on its own card.
                match proposed_plan(&text) {
                    Some((before, plan, after)) => {
                        b.answer(&id.clone(), id.clone(), at, before);
                        b.push(plan_item(format!("{id}:plan"), None, plan));
                        let rest = format!("{id}:after");
                        b.answer(&rest.clone(), rest, at, after);
                    }
                    None => b.answer(&id.clone(), id, at, &text),
                }
            }
            // Developer and system messages are Codex's instructions.
            _ => {}
        },
        "reasoning" => {
            let said = |key: &str| {
                (p[key].as_array().into_iter().flatten())
                    .filter_map(|s| s["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            let mut text = said("summary");
            if text.trim().is_empty() {
                text = said("content");
            }
            b.thinking(id, &text);
        }
        "function_call" => {
            let args = (p["arguments"].as_str())
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or_else(|| p["arguments"].clone());
            if name == "apply_patch"
                && let Some(text) = args["input"].as_str().or(args.as_str())
                && codex_patch(b, &id, &call, at, text)
            {
                return;
            }
            codex_call(b, id, call, at, name, &args);
        }
        "local_shell_call" => codex_call(b, id, call, at, "shell", &p["action"]),
        "custom_tool_call" => {
            let input = p["input"].as_str().unwrap_or_default();
            if name == "apply_patch" && codex_patch(b, &id, &call, at, input) {
                return;
            }
            // A script ("code mode"): its gist is the command or query it
            // runs, when one is plain to see. A patch's files show as
            // edits (`patch_apply_end`).
            let gist = match input.contains("*** Begin Patch") {
                true => "apply_patch".to_owned(),
                false => (js_str(input, "cmd").or_else(|| js_str(input, "q")))
                    .map_or_else(|| gist(input), |said| gist(&said)),
            };
            let item = Item::Tool {
                id,
                name: name.to_owned(),
                gist,
                input: input.to_owned(),
                output: None,
            };
            b.push_call(call, at, item);
        }
        "function_call_output" | "custom_tool_call_output" => {
            let Some(&(_, called)) = b.calls.get(&call) else {
                return;
            };
            let (text, failed, ms) = outcome(&p["output"]);
            let ms = ms.or_else(|| at?.checked_sub(called?));
            let text = cap(text.trim_end());
            b.fill(&call, ToolOut { text, failed, ms });
        }
        "web_search_call" => codex_search(b, id, p),
        // A snapshot of the work tree, a compaction's encrypted summary.
        "ghost_snapshot" | "compaction" => {}
        kind => {
            let text = cant_show(kind);
            b.push(Item::Note { id, text });
        }
    }
}

/// What only an `event_msg` line tells; the rest of them say again what
/// a `response_item` said, or are bookkeeping.
fn codex_event(b: &mut Builder, p: &Value) {
    let id = codex_id(b, p);
    match p["type"].as_str().unwrap_or_default() {
        "patch_apply_end" => codex_applied(b, &id, p),
        "web_search_end" => codex_search(b, id, p),
        "item_completed" => {
            let item = &p["item"];
            if item["type"] == "WebSearch" || item["kind"] == "web.search" {
                let id = codex_id(b, item);
                codex_search(b, id, item);
            }
        }
        "turn_aborted" => {
            let text = INTERRUPTED.to_owned();
            b.push(Item::Note { id, text });
        }
        "error" => {
            let text = p["message"]
                .as_str()
                .unwrap_or("An error")
                .trim()
                .to_owned();
            b.push(Item::Note { id, text });
        }
        _ => {}
    }
}

/// A tool call: a command run (`shell`, `exec_command`, a local shell
/// call's action), or another tool with its arguments `args`.
fn codex_call(
    b: &mut Builder,
    id: String,
    call: String,
    at: Option<u64>,
    name: &str,
    args: &Value,
) {
    let command = match &args["command"] {
        Value::Array(argv) => {
            let argv: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            match argv[..] {
                ["apply_patch", text] if codex_patch(b, &id, &call, at, text) => return,
                [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => {
                    Some(script.to_owned())
                }
                _ => Some(argv.join(" ")),
            }
        }
        Value::String(command) => Some(command.clone()),
        _ => args["cmd"].as_str().map(str::to_owned),
    };
    let item = Item::Tool {
        id,
        name: name.to_owned(),
        gist: command
            .as_deref()
            .map_or_else(|| tool_gist(args).unwrap_or_default(), gist),
        input: command.unwrap_or_else(|| serde_json::to_string_pretty(args).unwrap_or_default()),
        output: None,
    };
    b.push_call(call, at, item);
}

/// Patch `text` of call `call` as an edit per file, the call's result
/// going to the last; false when it changes no file.
fn codex_patch(b: &mut Builder, id: &str, call: &str, at: Option<u64>, text: &str) -> bool {
    let files = patch::files(text);
    let n = files.len();
    let last = n.saturating_sub(1);
    for (k, (path, diff)) in files.into_iter().enumerate() {
        let item = Item::Edit {
            id: if k == 0 {
                id.to_owned()
            } else {
                format!("{id}.{k}")
            },
            path,
            diff: capped(diff),
            output: None,
        };
        match k == last {
            true => b.push_call(call.to_owned(), at, item),
            false => b.push(item),
        }
    }
    n > 0
}

/// The files a patch changed, from `patch_apply_end` `p`: an edit each,
/// unless its call already showed them (an `apply_patch` call).
fn codex_applied(b: &mut Builder, id: &str, p: &Value) {
    let call = p["call_id"].as_str().unwrap_or_default();
    if let Some(&(i, _)) = b.calls.get(call)
        && matches!(b.items[i], Item::Edit { .. })
    {
        return;
    }
    let failed = p["success"] == false;
    let said = p[if failed { "stderr" } else { "stdout" }]
        .as_str()
        .unwrap_or_default();
    let out = ToolOut {
        text: cap(said.trim()),
        failed,
        ms: None,
    };
    for (k, (path, made)) in p["changes"].as_object().into_iter().flatten().enumerate() {
        // `{type: add, content}`, or older `{add: {content}}`.
        let (kind, made) = match made["type"].as_str() {
            Some(kind) => (kind, made),
            None => (made.as_object().and_then(|o| o.iter().next()))
                .map_or(("", made), |(kind, made)| (kind.as_str(), made)),
        };
        let text = |key: &str| made[key].as_str().unwrap_or_default();
        let diff = match kind {
            "add" => change("", text("content")),
            "delete" => change(text("content"), ""),
            _ => {
                let headed = format!("--- {path}\n+++ {path}\n{}", text("unified_diff"));
                patch::files(&headed)
                    .into_iter()
                    .flat_map(|(_, d)| d)
                    .collect()
            }
        };
        b.push(Item::Edit {
            id: format!("{id}.{k}"),
            path: made["move_path"].as_str().unwrap_or(path).to_owned(),
            diff: capped(diff),
            output: Some(out.clone()),
        });
    }
}

/// A web search, from `p`'s query or action; shown once, though Codex
/// may tell it both as a call and as an event, or as the script that
/// ran it.
fn codex_search(b: &mut Builder, id: String, p: &Value) {
    let action = &p["action"];
    let query = (p["query"].as_str().or(action["query"].as_str()))
        .or(action["url"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            let queries = action["queries"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str);
            Some(queries.collect::<Vec<_>>().join("; "))
        })
        .unwrap_or_default();
    let gist = gist(&query);
    let told = (b.items.iter().rev())
        .take_while(|i| !matches!(i, Item::User { .. }))
        .any(|i| matches!(i, Item::Tool { gist: g, .. } if *g == gist));
    if !told {
        let (name, input, output) = ("web search".to_owned(), query, None);
        b.push(Item::Tool {
            id,
            name,
            gist,
            input,
            output,
        });
    }
}

/// An id for payload `p`: its own, its call's, else its line's number.
fn codex_id(b: &Builder, p: &Value) -> String {
    (p["id"].as_str().or(p["call_id"].as_str()))
        .filter(|id| !id.is_empty())
        .map_or_else(|| format!("line{}", b.lines), str::to_owned)
}

/// `text` split around plan mode's `<proposed_plan>` block: what comes
/// before, the plan, and what after.
fn proposed_plan(text: &str) -> Option<(&str, &str, &str)> {
    let (before, rest) = text.split_once("<proposed_plan>")?;
    let (plan, after) = rest.split_once("</proposed_plan>")?;
    Some((before, plan.trim(), after))
}

/// The text parts of message `content` (`input_text`, `output_text`); an
/// image as "[image]".
fn parts(content: &Value) -> Vec<&str> {
    (content.as_array().into_iter().flatten())
        .filter_map(|part| match part["type"].as_str() {
            Some("input_image") => Some("[image]"),
            _ => part["text"].as_str(),
        })
        .collect()
}

/// What the user typed in message `content`, less what Codex adds: its
/// context and instructions (a part that is one `<tag>…</tag>`, or the
/// AGENTS.md text) and the files mentioned, pasted in `<context ref=…>`.
fn typed(content: &Value) -> String {
    let mut text: String = parts(content)
        .into_iter()
        .filter(|part| {
            let part = part.trim();
            let head = part
                .strip_prefix('<')
                .and_then(|t| t.split_once('>'))
                .map(|(h, _)| h);
            let wrapped = head.is_some_and(|head| {
                let name = head.split(' ').next().unwrap_or(head);
                let closes = |tag: &str| part.ends_with(&format!("</{tag}>"));
                !name.is_empty() && (closes(head) || closes(name))
            });
            !wrapped && !part.starts_with("# AGENTS.md instructions")
        })
        .collect();
    while let Some(start) = text.find("<context ref=")
        && let Some(len) = text[start..].find("</context>")
    {
        let before = text[..start].trim_end().len();
        text.replace_range(before..start + len + "</context>".len(), "");
    }
    text.trim().to_owned()
}

/// A tool's output `output` (a string, its text parts, or `{content,
/// success}`): its text, whether it failed, and how long it took when it
/// says.
fn outcome(output: &Value) -> (String, bool, Option<u64>) {
    let raw = match output {
        Value::String(text) => text.clone(),
        Value::Array(_) => parts(output).concat(),
        _ => output["content"].as_str().unwrap_or_default().to_owned(),
    };
    let mut failed = output["success"] == false;
    let ms = |secs: f64| (secs >= 0.0).then(|| (secs * 1000.0).round() as u64);
    // Older Codex: a JSON string of `{output, metadata: {exit_code,
    // duration_seconds}}`.
    if let Ok(json) = serde_json::from_str::<Value>(&raw)
        && let Some(text) = json["output"].as_str()
    {
        let meta = &json["metadata"];
        failed |= meta["exit_code"].as_i64().is_some_and(|c| c != 0);
        return (
            text.to_owned(),
            failed,
            meta["duration_seconds"].as_f64().and_then(ms),
        );
    }
    // Newer: a few lines on how it went ("Script completed", "Wall time 0.1
    // seconds", "Exit code: 0", "Process exited with code 1"), then
    // "Output:" and the output.
    let Some((head, body)) = (raw.split_once("\nOutput:"))
        .filter(|(head, _)| head.lines().count() <= 6 && head.contains("Wall time"))
    else {
        return (raw, failed, None);
    };
    let first = head.lines().next().unwrap_or_default().to_lowercase();
    failed |= first.contains("fail") || first.contains("error");
    let mut took = None;
    for line in head.lines() {
        if let Some(rest) = line.strip_prefix("Wall time") {
            let secs = rest.trim_start_matches(':').split_whitespace().next();
            took = secs.and_then(|s| s.parse().ok()).and_then(ms);
        }
        let code = (line.strip_prefix("Exit code:"))
            .or(line.strip_prefix("Process exited with code"))
            .and_then(|c| c.trim().parse::<i64>().ok());
        failed |= code.is_some_and(|c| c != 0);
    }
    let body = body.strip_prefix('\n').unwrap_or(body);
    (body.to_owned(), failed, took)
}

/// The string given for `key` in script `js` (`key:"…"` or `"key": '…'`).
fn js_str(js: &str, key: &str) -> Option<String> {
    js.match_indices(key).find_map(|(at, _)| {
        let before = js[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let rest = &js[at + key.len()..];
        let rest = rest.strip_prefix('"').unwrap_or(rest);
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        let quote = rest
            .chars()
            .next()
            .filter(|&q| matches!(q, '"' | '\'' | '`'))?;
        let mut escaped = false;
        let end = rest[1..].find(|c| {
            let end = !escaped && c == quote;
            escaped = !escaped && c == '\\';
            end
        })? + 1;
        match quote {
            '"' => serde_json::from_str(&rest[..=end]).ok(),
            _ => Some(rest[1..end].to_owned()),
        }
    })
}

/// The one line `text` says.
fn gist(text: &str) -> String {
    tool_gist(&Value::String(text.to_owned())).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        ai_work::AgentKind,
        chat::{DiffSide, parse::Reader},
    };

    /// Rollout lines by hand: `(type, payload)`, a second apart.
    fn rollout(lines: &[(&str, Value)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (n, (kind, payload)) in lines.iter().enumerate() {
            let at = format!("2026-01-02T03:04:{n:02}.000Z");
            let line = json!({ "timestamp": at, "type": kind, "payload": payload });
            out.extend(line.to_string().bytes());
            out.push(b'\n');
        }
        out
    }

    fn read(lines: &[(&str, Value)]) -> Vec<Item> {
        let mut reader = Reader::new(AgentKind::Codex);
        reader.feed(&rollout(lines));
        reader.items().to_vec()
    }

    fn item(payload: Value) -> (&'static str, Value) {
        ("response_item", payload)
    }

    fn event(payload: Value) -> (&'static str, Value) {
        ("event_msg", payload)
    }

    fn user(id: &str, parts: &[&str]) -> (&'static str, Value) {
        let content: Vec<Value> = (parts.iter())
            .map(|t| json!({ "type": "input_text", "text": t }))
            .collect();
        item(json!({ "type": "message", "id": id, "role": "user", "content": content }))
    }

    fn said(id: &str, text: &str) -> (&'static str, Value) {
        item(
            json!({ "type": "message", "id": id, "role": "assistant", "phase": "final_answer",
            "content": [{ "type": "output_text", "text": text }] }),
        )
    }

    fn out(item: &Item) -> &ToolOut {
        match item {
            Item::Tool {
                output: Some(o), ..
            }
            | Item::Edit {
                output: Some(o), ..
            } => o,
            other => panic!("no output: {other:?}"),
        }
    }

    fn sides(item: &Item) -> (&str, Vec<(DiffSide, &str)>) {
        let Item::Edit { path, diff, .. } = item else {
            panic!("not an edit: {item:?}");
        };
        let lines = diff.iter().map(|l| (l.side, l.text.as_str())).collect();
        (path, lines)
    }

    #[test]
    fn the_user_text_leaves_out_what_codex_adds() {
        let items = read(&[
            ("session_meta", json!({ "id": "s1", "cwd": "/w" })),
            ("turn_context", json!({ "cwd": "/w", "model": "m" })),
            item(json!({ "type": "message", "id": "d1", "role": "developer",
                "content": [{ "type": "input_text", "text": "<permissions instructions>x</permissions instructions>" }] })),
            user(
                "u0",
                &[
                    "# AGENTS.md instructions for /w\n\nBe nice.",
                    "<environment_context>\n  <cwd>/w</cwd>\n</environment_context>",
                    "<permissions instructions>\nAsk first.\n</permissions instructions>",
                ],
            ),
            user(
                "u1",
                &[
                    "Fix the bug in ",
                    "[@a.rs](file:///w/a.rs)\n<context ref=\"file:///w/a.rs\">\nfn a() {}\n</context>",
                    " please",
                ],
            ),
            event(
                json!({ "type": "user_message", "message": "Fix the bug in [@a.rs](file:///w/a.rs) please" }),
            ),
            event(
                json!({ "type": "item_completed", "item": { "type": "UserMessage", "id": "x1",
                "content": [{ "type": "text", "text": "Fix the bug" }] } }),
            ),
        ]);
        let want = Item::User {
            id: "u1".into(),
            at: iso_ms("2026-01-02T03:04:04.000Z"),
            text: "Fix the bug in [@a.rs](file:///w/a.rs) please".into(),
        };
        assert_eq!(items, [want]);
    }

    #[test]
    fn a_proposed_plan_shows_on_its_card_between_the_words() {
        let items = read(&[said(
            "m1",
            "Here it is.\n\n<proposed_plan>\n# Retry\n\n- wait\n</proposed_plan>\n\nSay go.",
        )]);
        let words = |i: usize| match &items[i] {
            Item::Assistant { blocks, .. } => blocks.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(*words(0), crate::md_blocks::blocks("Here it is."));
        let Item::Plan { id, text, path, .. } = &items[1] else {
            panic!("{items:?}")
        };
        assert_eq!(
            (id.as_str(), text.as_str(), path),
            ("m1:plan", "# Retry\n\n- wait", &None)
        );
        assert_eq!(*words(2), crate::md_blocks::blocks("Say go."));
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn answers_and_reasoning_show_once() {
        let items = read(&[
            event(json!({ "type": "agent_reasoning", "text": "**Looking**" })),
            item(
                json!({ "type": "reasoning", "id": "rs1", "encrypted_content": "zzz",
                "summary": [{ "type": "summary_text", "text": "**Looking**" }] }),
            ),
            item(json!({ "type": "reasoning", "id": "rs2", "summary": [] })),
            event(json!({ "type": "agent_message", "message": "Done.", "phase": "final_answer" })),
            said("m1", "Done."),
            event(
                json!({ "type": "item_completed", "item": { "type": "AgentMessage", "id": "m1",
                "content": [{ "type": "Text", "text": "Done." }] } }),
            ),
            event(json!({ "type": "token_count", "info": {} })),
            event(json!({ "type": "task_complete", "last_agent_message": "Done." })),
        ]);
        let thinking = Item::Thinking {
            id: "rs1".into(),
            text: "**Looking**".into(),
        };
        assert_eq!(items[0], thinking);
        let Item::Assistant { id, at, blocks } = &items[1] else {
            panic!("{items:?}");
        };
        assert_eq!(
            (id.as_str(), *at),
            ("m1", iso_ms("2026-01-02T03:04:04.000Z"))
        );
        assert_eq!(**blocks, crate::md_blocks::blocks("Done."));
        assert_eq!(items.len(), 2, "{items:?}");
    }

    #[test]
    fn a_script_pairs_with_its_output() {
        let script = r#"const r = await tools.exec_command({cmd:"ls -la \"a b\"","workdir":"/w"}); text(r.output);"#;
        let search = r#"const r = await tools.web__run({search_query:[{q:"rust docs"}]}); text(r)"#;
        let items = read(&[
            item(
                json!({ "type": "custom_tool_call", "id": "ctc1", "call_id": "c1", "name": "exec", "input": script }),
            ),
            item(
                json!({ "type": "custom_tool_call_output", "id": "o1", "call_id": "c1", "output": [
                    { "type": "input_text", "text": "Script completed\nWall time 0.4 seconds\nOutput:\n" },
                    { "type": "input_text", "text": "a\nb\n" },
                ]}),
            ),
            item(
                json!({ "type": "custom_tool_call", "id": "ctc2", "call_id": "c2", "name": "exec", "input": search }),
            ),
            // The search the script made: shown by the script already.
            event(json!({ "type": "web_search_end", "call_id": "exec-1", "query": "rust docs" })),
            item(json!({ "type": "custom_tool_call_output", "call_id": "c2",
                "output": "Script failed\nWall time 1.0 seconds\nOutput:\nTypeError: x" })),
            item(
                json!({ "type": "custom_tool_call", "id": "ctc3", "call_id": "c3", "name": "exec", "input": "1 + 1" }),
            ),
            item(json!({ "type": "custom_tool_call_output", "call_id": "c3", "output": "2" })),
        ]);
        assert_eq!(items.len(), 3, "{items:?}");
        let Item::Tool {
            id,
            name,
            gist,
            input,
            ..
        } = &items[0]
        else {
            panic!()
        };
        assert_eq!(
            (id.as_str(), name.as_str(), gist.as_str(), input.as_str()),
            ("ctc1", "exec", "ls -la \"a b\"", script)
        );
        let ok = ToolOut {
            text: "a\nb".into(),
            failed: false,
            ms: Some(400),
        };
        assert_eq!(out(&items[0]), &ok);
        assert!(matches!(&items[1], Item::Tool { gist, .. } if gist == "rust docs"));
        let failed = ToolOut {
            text: "TypeError: x".into(),
            failed: true,
            ms: Some(1000),
        };
        assert_eq!(out(&items[1]), &failed);
        assert!(matches!(&items[2], Item::Tool { gist, .. } if gist == "1 + 1"));
        // No wall time said: from the timestamps.
        assert_eq!(out(&items[2]).ms, Some(1000));
    }

    #[test]
    fn older_shell_calls_pair_with_their_output() {
        let shell = json!({ "command": ["bash", "-lc", "cargo test"], "workdir": "/w" });
        let exec = json!({ "cmd": "git status", "workdir": "/w" });
        let result = json!({ "output": "1 failed\n",
            "metadata": { "exit_code": 101, "duration_seconds": 1.5 } });
        let plan = json!({ "plan": [{ "step": "Read", "status": "done" }], "explanation": "Plan" });
        let items = read(&[
            item(
                json!({ "type": "function_call", "id": "fc1", "call_id": "c1", "name": "shell",
                "arguments": shell.to_string() }),
            ),
            item(
                json!({ "type": "function_call_output", "call_id": "c1", "output": result.to_string() }),
            ),
            item(
                json!({ "type": "local_shell_call", "call_id": "c2", "status": "completed",
                "action": { "type": "exec", "command": ["ls", "-a"] } }),
            ),
            item(json!({ "type": "function_call_output", "call_id": "c2",
                "output": "Exit code: 0\nWall time: 0.2 seconds\nOutput:\n.\n..\n" })),
            item(
                json!({ "type": "function_call", "call_id": "c3", "name": "exec_command",
                "arguments": exec.to_string() }),
            ),
            item(json!({ "type": "function_call_output", "call_id": "c3",
                "output": "Chunk ID: 1\nWall time: 0.0516 seconds\nProcess exited with code 128\nOriginal token count: 3\nOutput:\nfatal\n" })),
            item(
                json!({ "type": "function_call", "call_id": "c4", "name": "update_plan",
                "arguments": plan.to_string() }),
            ),
            // An output whose call was before reading began.
            item(json!({ "type": "function_call_output", "call_id": "gone", "output": "x" })),
        ]);
        assert_eq!(items.len(), 4, "{items:?}");
        let tool = |item: &Item| match item {
            Item::Tool {
                id,
                name,
                gist,
                input,
                ..
            } => [id.clone(), name.clone(), gist.clone(), input.clone()],
            other => panic!("{other:?}"),
        };
        assert_eq!(
            tool(&items[0]),
            ["fc1", "shell", "cargo test", "cargo test"]
        );
        let failed = ToolOut {
            text: "1 failed".into(),
            failed: true,
            ms: Some(1500),
        };
        assert_eq!(out(&items[0]), &failed);
        assert_eq!(tool(&items[1]), ["c2", "shell", "ls -a", "ls -a"]);
        let ok = ToolOut {
            text: ".\n..".into(),
            failed: false,
            ms: Some(200),
        };
        assert_eq!(out(&items[1]), &ok);
        assert_eq!(tool(&items[2])[2], "git status");
        let o = out(&items[2]);
        assert_eq!((o.text.as_str(), o.failed, o.ms), ("fatal", true, Some(52)));
        let [_, name, gist, input] = tool(&items[3]);
        assert_eq!((name.as_str(), gist.as_str()), ("update_plan", "Plan"));
        assert!(input.contains("\"step\": \"Read\""), "{input}");
    }

    const PATCH: &str = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n*** Add File: b.md\n+# B\n*** End Patch\n";

    #[test]
    fn an_apply_patch_call_is_an_edit_per_file_and_not_again_when_applied() {
        let applied = json!({ "type": "patch_apply_end", "call_id": "c1", "success": true,
        "stdout": "Success.", "stderr": "", "changes": {
            "src/a.rs": { "type": "update", "unified_diff": "@@ -1 +1 @@\n-old\n+new\n" },
            "b.md": { "type": "add", "content": "# B\n" },
        }});
        let done = json!({ "output": "Success. Updated the following files:\nM src/a.rs\nA b.md\n",
            "metadata": { "exit_code": 0, "duration_seconds": 0.0 } });
        let items = read(&[
            item(
                json!({ "type": "custom_tool_call", "id": "ctc1", "call_id": "c1",
                "name": "apply_patch", "input": PATCH }),
            ),
            event(applied),
            item(
                json!({ "type": "custom_tool_call_output", "call_id": "c1", "output": done.to_string() }),
            ),
            // gpt-oss's form, and the shell's older one.
            item(
                json!({ "type": "function_call", "id": "fc2", "call_id": "c2", "name": "apply_patch",
                "arguments": json!({ "input": PATCH }).to_string() }),
            ),
            item(
                json!({ "type": "function_call", "id": "fc3", "call_id": "c3", "name": "shell",
                "arguments": json!({ "command": ["apply_patch", PATCH] }).to_string() }),
            ),
        ]);
        let ids: Vec<&str> = items.iter().map(Item::id).collect();
        assert_eq!(ids, ["ctc1", "ctc1.1", "fc2", "fc2.1", "fc3", "fc3.1"]);
        use DiffSide::*;
        assert_eq!(
            sides(&items[0]),
            ("src/a.rs", vec![(Removed, "old"), (Added, "new")])
        );
        assert_eq!(sides(&items[1]), ("b.md", vec![(Added, "# B")]));
        assert!(matches!(&items[0], Item::Edit { output: None, .. }));
        assert_eq!(out(&items[1]).ms, Some(0));
        assert!(out(&items[1]).text.starts_with("Success. Updated"));
    }

    #[test]
    fn a_patch_applied_from_a_script_shows_its_edits() {
        let script = r#"const patch = "*** Begin Patch\n..."; await tools.apply_patch(patch);"#;
        let items = read(&[
            item(
                json!({ "type": "custom_tool_call", "id": "ctc1", "call_id": "c1", "name": "exec", "input": script }),
            ),
            event(
                json!({ "type": "patch_apply_end", "call_id": "exec-1", "success": true,
                "stdout": "Success. Updated the following files:\nA new.txt\n", "stderr": "", "changes": {
                    "/w/new.txt": { "type": "add", "content": "one\ntwo\n" },
                    "/w/old.rs": { "type": "update", "unified_diff": "@@ -1,2 +1,2 @@\n keep\n-a\n+b\n",
                        "move_path": "/w/moved.rs" },
                    "/w/gone.txt": { "type": "delete", "content": "bye\n" },
                }}),
            ),
            // Older Codex: `{add: {content}}`.
            event(
                json!({ "type": "patch_apply_end", "call_id": "exec-2", "success": false,
                "stdout": "", "stderr": "no such file", "changes": { "/w/x": { "add": { "content": "x" } } } }),
            ),
            item(json!({ "type": "custom_tool_call_output", "call_id": "c1",
                "output": "Script completed\nWall time 0.0 seconds\nOutput:\n" })),
        ]);
        let ids: Vec<&str> = items.iter().map(Item::id).collect();
        assert_eq!(
            ids,
            ["ctc1", "exec-1.0", "exec-1.1", "exec-1.2", "exec-2.0"]
        );
        use DiffSide::*;
        let mut edits: Vec<_> = items[1..4].iter().map(sides).collect();
        edits.sort_by_key(|(path, _)| *path);
        assert_eq!(
            edits,
            [
                ("/w/gone.txt", vec![(Removed, "bye")]),
                (
                    "/w/moved.rs",
                    vec![(Context, "keep"), (Removed, "a"), (Added, "b")]
                ),
                ("/w/new.txt", vec![(Added, "one"), (Added, "two")]),
            ]
        );
        assert!(!out(&items[1]).failed);
        assert_eq!(sides(&items[4]), ("/w/x", vec![(Added, "x")]));
        assert_eq!(
            (out(&items[4]).failed, out(&items[4]).text.as_str()),
            (true, "no such file")
        );
        assert_eq!(out(&items[0]).text, "");
        assert!(matches!(&items[0], Item::Tool { gist, .. } if gist == "apply_patch"));
    }

    #[test]
    fn a_web_search_shows_once() {
        let items = read(&[
            user("u1", &["look it up"]),
            item(
                json!({ "type": "web_search_call", "id": "ws1", "status": "completed",
                "action": { "type": "search", "query": "rust lifetimes" } }),
            ),
            event(
                json!({ "type": "web_search_end", "call_id": "ws1", "query": "rust lifetimes",
                "action": { "type": "search", "query": "rust lifetimes" } }),
            ),
            // Only told as an event (from a script), the newer way.
            event(
                json!({ "type": "item_completed", "item": { "type": "Extension", "kind": "web.search",
                "id": "exec-9", "query": "tokio select",
                "action": { "type": "search", "queries": ["tokio select", "async"] } } }),
            ),
            user("u2", &["again"]),
            event(
                json!({ "type": "web_search_end", "call_id": "exec-10", "query": "rust lifetimes" }),
            ),
        ]);
        let searches: Vec<(&str, &str)> = (items.iter())
            .filter_map(|i| match i {
                Item::Tool { id, name, gist, .. } if name == "web search" => {
                    Some((id.as_str(), gist.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            searches,
            [
                ("ws1", "rust lifetimes"),
                ("exec-9", "tokio select"),
                ("exec-10", "rust lifetimes")
            ]
        );
    }

    #[test]
    fn notes_and_skips() {
        let items = read(&[
            ("world_state", json!({ "x": 1 })),
            ("token_usage_record", json!({ "x": 1 })),
            event(json!({ "type": "task_started", "turn_id": "t1" })),
            event(json!({ "type": "thread_settings_applied", "thread_settings": {} })),
            event(json!({ "type": "turn_aborted", "reason": "interrupted" })),
            ("compacted", json!({ "message": "summary" })),
            item(json!({ "type": "image_generation_call", "id": "ig1" })),
            item(json!({ "type": "ghost_snapshot", "ghost_commit": {} })),
            event(json!({ "type": "something_new", "id": "e1" })),
            event(json!({ "type": "error", "message": "Quota exceeded" })),
        ]);
        let note = |id: &str, text: &str| Item::Note {
            id: id.into(),
            text: text.into(),
        };
        let want = [
            note("line5", "Interrupted by the user"),
            note("line6", "The chat was compacted"),
            note(
                "ig1",
                "A \"image_generation_call\" line gerdr can't show yet",
            ),
            note("line10", "Quota exceeded"),
        ];
        assert_eq!(items, want);
    }

    #[test]
    fn reading_again_gives_the_same_items() {
        let lines = rollout(&[
            user("u1", &["hi"]),
            item(json!({ "type": "message", "role": "user",
                "content": [{ "type": "input_text", "text": "no id" }] })),
            item(
                json!({ "type": "custom_tool_call", "id": "ctc1", "call_id": "c1",
                "name": "apply_patch", "input": PATCH }),
            ),
            item(
                json!({ "type": "function_call", "call_id": "c2", "name": "shell",
                "arguments": json!({ "command": ["true"] }).to_string() }),
            ),
            item(json!({ "type": "function_call_output", "call_id": "c2", "output": "ok" })),
            event(
                json!({ "type": "patch_apply_end", "call_id": "exec-1", "success": true, "stdout": "",
                "changes": { "a": { "type": "add", "content": "a" } } }),
            ),
            event(json!({ "type": "turn_aborted", "reason": "interrupted" })),
            said("m1", "ok"),
        ]);
        let whole = {
            let mut reader = Reader::new(AgentKind::Codex);
            reader.feed(&lines);
            reader.items().to_vec()
        };
        let ids: Vec<&str> = whole.iter().map(Item::id).collect();
        assert_eq!(
            ids,
            [
                "u1", "line2", "ctc1", "ctc1.1", "c2", "exec-1.0", "line7", "m1"
            ]
        );
        for cut in [1, 40, lines.len() / 2, lines.len() - 1] {
            let mut reader = Reader::new(AgentKind::Codex);
            reader.feed(&lines[..cut]);
            reader.feed(&lines[cut..]);
            assert_eq!(reader.items(), whole, "cut at {cut}");
        }
    }

    #[test]
    fn a_script_string_reads_with_its_escapes() {
        assert_eq!(js_str(r#"f({cmd:"a\nb"})"#, "cmd").as_deref(), Some("a\nb"));
        assert_eq!(js_str(r#"f({"cmd": 'it'})"#, "cmd").as_deref(), Some("it"));
        assert_eq!(
            js_str(r#"f({search_query:[{q:"x"}]})"#, "q").as_deref(),
            Some("x")
        );
        assert_eq!(js_str(r#"f({xcmd:"no"})"#, "cmd"), None);
    }
}
