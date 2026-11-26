//! A Cursor chat, read out of its `store.db` a message at a time.
//!
//! Messages are the AI SDK's: `user`, `assistant` (text, reasoning and tool
//! calls), `tool` (their results). Items are named by the message's place:
//! assistant message ids repeat within a chat, and user messages have none.

use serde_json::Value;

use crate::{
    chat::{Item, ToolOut, parse::gist},
    integrations::{
        Parser,
        common::build::{Builder, COMPACTED, cap, change, edit_or_tool, input_text, tag, texts},
    },
};

/// A reader of Cursor's: it keeps nothing of its own.
pub(crate) struct Cursor;

impl Parser for Cursor {
    /// Read the chat's messages, each named by its place.
    fn rows(&mut self, b: &mut Builder, rows: &mut dyn Iterator<Item = &Value>) {
        rows.enumerate().for_each(|(n, row)| cursor_row(b, n, row));
    }
}

/// Read message `row`, the `n`th of the chat (see `chat::parse::snapshot`).
fn cursor_row(b: &mut Builder, n: usize, row: &Value) {
    match row["role"].as_str().unwrap_or_default() {
        "user" => {
            let text = texts(&row["content"]);
            let text = text.trim();
            let id = format!("cursor{n}");
            if let Some(typed) = tag(text, "user_query") {
                let text = typed.trim().to_owned();
                if !text.is_empty() {
                    b.push(Item::User { id, at: None, text });
                }
            } else if text.contains("<conversation_summary>") {
                let text = COMPACTED.to_owned();
                b.push(Item::Note { id, text });
            } else if !text.is_empty() && !text.starts_with('<') {
                // What Cursor adds for the model comes in tags alone.
                let text = text.to_owned();
                b.push(Item::User { id, at: None, text });
            }
        }
        "assistant" => {
            let message = format!("cursor{n}");
            for (k, part) in row["content"].as_array().into_iter().flatten().enumerate() {
                let id = format!("cursor{n}.{k}");
                match part["type"].as_str() {
                    Some("text") => {
                        b.answer(&message, id, None, part["text"].as_str().unwrap_or(""))
                    }
                    Some("reasoning") => b.thinking(id, part["text"].as_str().unwrap_or("")),
                    Some("tool-call") => cursor_call(b, part, id),
                    _ => {}
                }
            }
        }
        "tool" => {
            let done = &row["providerOptions"]["cursor"]["highLevelToolCallResult"];
            let said = &done["output"];
            // A shell command that exited non-zero is a "failure".
            let failed =
                done["isError"] == true || !said["error"].is_null() || !said["failure"].is_null();
            for part in row["content"].as_array().into_iter().flatten() {
                let Some(call) = part["toolCallId"].as_str() else {
                    continue;
                };
                let mut text = match &part["result"] {
                    Value::String(text) => text.clone(),
                    Value::Null => String::new(),
                    other => serde_json::to_string_pretty(other).unwrap_or_default(),
                };
                if text.trim().is_empty() && failed {
                    let error = &said["error"];
                    let why = (error["errorMessage"].as_str())
                        .or(error["error"].as_str())
                        .or(error.as_str());
                    text = why.unwrap_or_default().to_owned();
                }
                let out = ToolOut {
                    text: cap(&text),
                    failed,
                    ms: None,
                };
                b.fill(call, out);
            }
        }
        // system: the instructions.
        _ => {}
    }
}

/// Tool call `part`: an edit with its change, or a tool with its gist.
fn cursor_call(b: &mut Builder, part: &Value, id: String) {
    let call = part["toolCallId"].as_str().map_or(id, str::to_owned);
    let args = &part["args"];
    // Plan mode hands its plan over; its file is named in the result.
    if part["toolName"] == "CreatePlan"
        && let Some(item) =
            (args["plan"].as_str()).and_then(|plan| b.plan(call.clone(), None, plan))
    {
        b.push_call(call, None, item);
        return;
    }
    let text = |key: &str| args[key].as_str().unwrap_or("");
    let diff = match part["toolName"].as_str().unwrap_or("tool") {
        "StrReplace" => Some(change(text("old_string"), text("new_string"))),
        "Write" => Some(change("", text("contents"))),
        _ => None,
    };
    let item = edit_or_tool(call.clone(), args["path"].as_str(), diff, || {
        let mut name = part["toolName"].as_str().unwrap_or("tool");
        let mut input = args;
        // A tool of an MCP server or Cursor's own, called by name.
        if name == "CallDynamicTool" {
            name = args["toolName"].as_str().unwrap_or(name);
            input = &args["arguments"];
        }
        (name.to_owned(), gist(input), input_text(input))
    });
    b.push_call(call, None, item);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        ai_work::AgentKind,
        chat::{DiffLine, DiffSide, parse::snapshot},
        md_blocks,
    };

    fn read(rows: &[Value]) -> Vec<Item> {
        snapshot(AgentKind::Cursor, rows)
    }

    fn user(content: Value) -> Value {
        json!({ "role": "user", "content": content })
    }

    fn said(id: &str, parts: Value) -> Value {
        json!({ "role": "assistant", "id": id, "content": parts })
    }

    fn call(id: &str, name: &str, args: Value) -> Value {
        json!({ "type": "tool-call", "toolCallId": id, "toolName": name, "args": args })
    }

    fn result(id: &str, name: &str, result: &str, output: Value, error: bool) -> Value {
        json!({ "role": "tool", "id": id,
            "content": [{ "type": "tool-result", "toolCallId": id, "toolName": name, "result": result }],
            "providerOptions": { "cursor": { "highLevelToolCallResult": { "output": output, "isError": error } } } })
    }

    #[test]
    fn a_created_plan_shows_on_its_card() {
        let plan = "# Merge\n\n1. fetch\n2. merge\n";
        let items = read(&[
            said(
                "a1",
                json!([call(
                    "c1",
                    "CreatePlan",
                    json!({ "name": "Merge", "plan": plan, "todos": [] })
                )]),
            ),
            result(
                "c1",
                "CreatePlan",
                "Plan file created",
                json!({ "success": {} }),
                false,
            ),
        ]);
        let Item::Plan {
            id, text, output, ..
        } = &items[0]
        else {
            panic!("{items:?}")
        };
        assert_eq!((id.as_str(), text.as_str()), ("c1", plan));
        assert_eq!(output.as_ref().map(|o| o.failed), Some(false));
    }

    #[test]
    fn what_the_user_typed_shows_and_context_does_not() {
        let items = read(&[
            json!({ "role": "system", "content": "You are" }),
            user(json!(
                "<user_info>\nOS: linux\n</user_info>\n<rules>\nbe nice\n</rules>"
            )),
            user(
                json!([{ "type": "text", "text": "<timestamp>now</timestamp>\n<user_query>\nfix the bug\n</user_query>" }]),
            ),
            user(json!("plain words")),
            user(json!(
                "<conversation_summary>\nall of it\n</conversation_summary>"
            )),
            // An Enter on an empty prompt.
            user(json!("<user_query>\n\n</user_query>")),
        ]);
        let want = vec![
            Item::User {
                id: "cursor2".into(),
                at: None,
                text: "fix the bug".into(),
            },
            Item::User {
                id: "cursor3".into(),
                at: None,
                text: "plain words".into(),
            },
            Item::Note {
                id: "cursor4".into(),
                text: "Conversation compacted".into(),
            },
        ];
        assert_eq!(items, want);
    }

    #[test]
    fn answers_and_thinking() {
        let items = read(&[said(
            "1",
            json!([
                { "type": "reasoning", "text": "", "signature": "x" },
                { "type": "reasoning", "text": "Hmm." },
                { "type": "text", "text": "One." },
                { "type": "text", "text": "Two." },
            ]),
        )]);
        let want = vec![
            Item::Thinking {
                id: "cursor0.1".into(),
                text: "Hmm.".into(),
            },
            Item::Assistant {
                id: "cursor0.2".into(),
                at: None,
                blocks: md_blocks::blocks("One.\n\nTwo.").into(),
            },
        ];
        assert_eq!(items, want);
    }

    #[test]
    fn results_pair_with_their_calls() {
        let items = read(&[
            said(
                "m1",
                json!([
                    call(
                        "tool_1\nfc_1",
                        "Shell",
                        json!({ "command": "cargo test", "description": "Test" })
                    ),
                    call(
                        "tool_2\nfc_2",
                        "Grep",
                        json!({ "pattern": "fn main", "path": "/r" })
                    ),
                    call("tool_3\nfc_3", "Read", json!({ "path": "/r/gone.rs" })),
                    call(
                        "tool_4\nfc_4",
                        "Glob",
                        json!({ "glob_pattern": "*.rs", "target_directory": "/r" })
                    ),
                ]),
            ),
            result(
                "tool_1\nfc_1",
                "Shell",
                "exit 1",
                json!({ "failure": { "exitCode": 1 } }),
                false,
            ),
            result(
                "tool_2\nfc_2",
                "Grep",
                "src/main.rs",
                json!({ "success": {} }),
                false,
            ),
            result(
                "tool_3\nfc_3",
                "Read",
                "",
                json!({ "error": { "errorMessage": "No such file" } }),
                true,
            ),
        ]);
        let got: Vec<_> = (items.iter())
            .map(|item| match item {
                Item::Tool {
                    id,
                    name,
                    gist,
                    output,
                    ..
                } => {
                    let out = output.as_ref().map(|o| (o.text.as_str(), o.failed));
                    (id.as_str(), name.as_str(), gist.as_str(), out)
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    "tool_1\nfc_1",
                    "Shell",
                    "cargo test",
                    Some(("exit 1", true))
                ),
                (
                    "tool_2\nfc_2",
                    "Grep",
                    "fn main",
                    Some(("src/main.rs", false))
                ),
                (
                    "tool_3\nfc_3",
                    "Read",
                    "/r/gone.rs",
                    Some(("No such file", true))
                ),
                ("tool_4\nfc_4", "Glob", "*.rs", None),
            ]
        );
    }

    #[test]
    fn str_replace_and_write_are_edits() {
        use DiffSide::{Added, Removed};
        let line = |side, text: &str| DiffLine {
            side,
            text: text.into(),
        };
        let items = read(&[
            said(
                "m1",
                json!([
                    call(
                        "t1",
                        "StrReplace",
                        json!({ "path": "/r/a.rs", "old_string": "a", "new_string": "b" })
                    ),
                    call(
                        "t2",
                        "Write",
                        json!({ "path": "/r/b.rs", "contents": "x\ny" })
                    ),
                ]),
            ),
            result(
                "t1",
                "StrReplace",
                "The file was updated",
                json!({ "success": {} }),
                false,
            ),
        ]);
        let want = vec![
            Item::Edit {
                id: "t1".into(),
                path: "/r/a.rs".into(),
                diff: vec![line(Removed, "a"), line(Added, "b")],
                output: Some(ToolOut {
                    text: "The file was updated".into(),
                    failed: false,
                    ms: None,
                }),
            },
            Item::Edit {
                id: "t2".into(),
                path: "/r/b.rs".into(),
                diff: vec![line(Added, "x"), line(Added, "y")],
                output: None,
            },
        ];
        assert_eq!(items, want);
    }

    #[test]
    fn a_dynamic_tool_goes_by_its_own_name() {
        let args = json!({ "namespace": "cursor", "toolName": "TodoWrite",
            "arguments": { "todos": [{ "id": "1", "content": "Do it", "status": "pending" }], "merge": false } });
        let items = read(&[said("m1", json!([call("t1", "CallDynamicTool", args)]))]);
        let Item::Tool { name, input, .. } = &items[0] else {
            panic!("{items:?}")
        };
        assert_eq!(name, "TodoWrite");
        assert!(
            input.contains("Do it") && !input.contains("namespace"),
            "{input}"
        );
    }

    #[test]
    fn reading_again_gives_the_same_items() {
        let rows = [
            user(json!("<user_query>go</user_query>")),
            said(
                "1",
                json!([{ "type": "text", "text": "Ok." }, call("t1", "Shell", json!({ "command": "ls" }))]),
            ),
            result("t1", "Shell", "a", json!({ "success": {} }), false),
            said("1", json!([{ "type": "text", "text": "Done." }])),
        ];
        let first = read(&rows);
        assert_eq!(first, read(&rows));
        let ids: Vec<&str> = first.iter().map(Item::id).collect();
        // The second message's id repeats the first's: its place tells them apart.
        assert_eq!(ids, ["cursor0", "cursor1.0", "t1", "cursor3.0"]);
    }
}
