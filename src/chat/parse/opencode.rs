//! OpenCode's chat, read out of `opencode.db` a row at a time.
//!
//! Two schemas: a `message` row with its `parts` (v1), or a
//! `session_message` row whose `data` holds it all (v2, newer sessions). A
//! tool part holds its call and its result together, so it is one item,
//! made whole each time the rows are read.

use std::sync::Arc;

use serde_json::Value;

use super::{DiffLine, Item, ToolOut, patch};
use crate::integrations::{
    Parser,
    common::build::{
        self, Builder, COMPACTED, INTERRUPTED, cap, capped, change, input_text, texts,
    },
};

/// A reader of OpenCode's: it keeps nothing of its own.
pub(crate) struct OpenCode;

impl Parser for OpenCode {
    fn rows(&mut self, b: &mut Builder, rows: &mut dyn Iterator<Item = &Value>) {
        rows.for_each(|row| opencode_row(b, row));
    }
}

/// Read message row `row` of an OpenCode session (see `super::snapshot`).
fn opencode_row(b: &mut Builder, row: &Value) {
    let id = row["id"].as_str().unwrap_or_default();
    let data = &row["data"];
    let at = data["time"]["created"]
        .as_u64()
        .or(row["time_created"].as_u64());
    // v2 rows have a type; v1 rows a role in their data.
    let kind = (row["type"].as_str()).or(data["role"].as_str());
    match kind.unwrap_or_default() {
        "user" if row.get("parts").is_some() => {
            for part in row["parts"].as_array().into_iter().flatten() {
                let pid = part["id"].as_str().unwrap_or(id).to_owned();
                let part = &part["data"];
                match part["type"].as_str() {
                    Some("text") if part["synthetic"] != true => {
                        let text = part["text"].as_str().unwrap_or("").trim().to_owned();
                        if !text.is_empty() {
                            b.push(Item::User { id: pid, at, text });
                        }
                    }
                    Some("compaction") => b.push(compacted(pid)),
                    _ => {}
                }
            }
        }
        "user" => {
            let text = data["text"].as_str().unwrap_or("").trim().to_owned();
            if !text.is_empty() {
                b.push(Item::User {
                    id: id.to_owned(),
                    at,
                    text,
                });
            }
        }
        "assistant" => {
            // v1 parts are rows of their own; v2 content is in the data.
            let parts: Vec<(String, &Value)> = match row.get("parts") {
                Some(parts) => (parts.as_array().into_iter().flatten())
                    .map(|p| (p["id"].as_str().unwrap_or(id).to_owned(), &p["data"]))
                    .collect(),
                None => (data["content"].as_array().into_iter().flatten())
                    .enumerate()
                    .map(|(k, p)| (format!("{id}.{k}"), p))
                    .collect(),
            };
            for (pid, part) in parts {
                match part["type"].as_str() {
                    // The summary a compaction made, noted where it was asked.
                    Some("text") if data["summary"] == true => {}
                    Some("text") => b.answer(id, pid, at, part["text"].as_str().unwrap_or("")),
                    Some("reasoning") => b.thinking(pid, part["text"].as_str().unwrap_or("")),
                    Some("tool") => opencode_tool(b, pid, part),
                    Some("compaction") => b.push(compacted(pid)),
                    _ => {}
                }
            }
            let error = &data["error"];
            if !error.is_null() {
                let aborted = error["name"] == "MessageAbortedError" || error["type"] == "aborted";
                let text = match aborted {
                    true => INTERRUPTED.to_owned(),
                    false => (error["data"]["message"].as_str())
                        .or(error["message"].as_str())
                        .or(error["name"].as_str())
                        .unwrap_or("The agent stopped on an error")
                        .to_owned(),
                };
                b.push(Item::Note {
                    id: format!("{id}.error"),
                    text,
                });
            }
        }
        "compaction" => b.push(compacted(id.to_owned())),
        "model-switched" => {
            if let Some(model) = data["model"]["id"].as_str() {
                let text = format!("Set model to {model}");
                b.push(Item::Note {
                    id: id.to_owned(),
                    text,
                });
            }
        }
        // system (instructions), synthetic (what OpenCode tells the
        // model), idle (an aborted turn's error notes it already).
        _ => {}
    }
}

/// Tool part `part` (v1's `{tool, callID, state}` or v2's `{id, name,
/// state, time}`): an edit per file it changed, or a tool.
fn opencode_tool(b: &mut Builder, pid: String, part: &Value) {
    let name = (part["tool"].as_str())
        .or(part["name"].as_str())
        .unwrap_or("tool");
    let id = (part["callID"].as_str())
        .or(part["id"].as_str())
        .map_or(pid, str::to_owned);
    let state = &part["state"];
    let input = &state["input"];
    let output = opencode_out(part);
    // A `task` call whose child session was read (`tail::db`): its work
    // under one card, as Claude's subagents.
    if let Some(rows) = part["subagent"].as_array() {
        let title = Some(gist(input)).filter(|title| !title.is_empty());
        return b.push(Item::Subagent {
            id: format!("agent:{id}"),
            title: title.unwrap_or_else(|| name.to_owned()),
            items: Arc::new({
                let mut b = Builder::default();
                OpenCode.rows(&mut b, &mut rows.iter());
                b.items
            }),
            output,
        });
    }
    let text = |key: &str| input[key].as_str().unwrap_or("");
    let path = (input["filePath"].as_str()).or(input["path"].as_str());
    let diffs: Vec<(String, Vec<DiffLine>)> = match (name, path) {
        ("edit", Some(path)) => vec![(
            path.to_owned(),
            change(text("oldString"), text("newString")),
        )],
        ("write", Some(path)) => vec![(path.to_owned(), change("", text("content")))],
        ("apply_patch" | "patch", _) => {
            let made: Vec<_> = (state["metadata"]["files"].as_array().into_iter().flatten())
                .flat_map(|f| {
                    let lines = patch::files(f["patch"].as_str().unwrap_or("")).into_iter();
                    let lines = lines.flat_map(|(_, lines)| lines).collect();
                    let path = (f["filePath"].as_str()).or(f["file"].as_str())?;
                    Some((path.to_owned(), lines))
                })
                .collect();
            // Not run yet, or it failed: the patch as asked.
            match made.is_empty() {
                true => patch::files(text("patchText")),
                false => made,
            }
        }
        _ => Vec::new(),
    };
    if diffs.is_empty() {
        return b.push(Item::Tool {
            id,
            name: name.to_owned(),
            gist: gist(input),
            input: input_text(input),
            output,
        });
    }
    for (k, (path, diff)) in diffs.into_iter().enumerate() {
        let id = match k {
            0 => id.clone(),
            k => format!("{id}.{k}"),
        };
        let output = output.clone();
        b.push(Item::Edit {
            id,
            path,
            diff: capped(diff),
            output,
        });
    }
}

/// What tool part `part` gave: its output or error, and how long it took;
/// none while it is pending or running.
fn opencode_out(part: &Value) -> Option<ToolOut> {
    let state = &part["state"];
    let failed = match state["status"].as_str() {
        Some("completed") => false,
        Some("error") => true,
        _ => return None,
    };
    let text = match failed {
        true => (state["error"].as_str())
            .or(state["error"]["message"].as_str())
            .unwrap_or("")
            .to_owned(),
        false => state["output"]
            .as_str()
            .map_or_else(|| texts(&state["content"]), str::to_owned),
    };
    // v1 times are the state's; v2's the part's, from when it ran.
    let time = &state["time"];
    let ms = (time["end"].as_u64())
        .zip(time["start"].as_u64())
        .or_else(|| {
            let time = &part["time"];
            let start = time["ran"].as_u64().or(time["created"].as_u64());
            time["completed"].as_u64().zip(start)
        })
        .and_then(|(end, start)| end.checked_sub(start));
    Some(ToolOut {
        text: cap(&text),
        failed,
        ms,
    })
}

fn compacted(id: String) -> Item {
    let text = COMPACTED.to_owned();
    Item::Note { id, text }
}

/// What OpenCode's and Cursor's tools name their arguments, their gist
/// first.
const GIST_KEYS: [&str; 10] = [
    "command",
    "filePath",
    "pattern",
    "glob_pattern",
    "path",
    "target_directory",
    "url",
    "query",
    "description",
    "name",
];

/// The one line a tool's `input` says: its command, file, pattern; OpenCode's
/// and Cursor's keys first (`filePath`, `glob_pattern`), else as Claude's.
pub(crate) fn gist(input: &Value) -> String {
    build::gist(input, &GIST_KEYS)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        ai_work::AgentKind,
        chat::{DiffSide, parse::snapshot},
        md_blocks,
    };

    fn read(rows: &[Value]) -> Vec<Item> {
        snapshot(AgentKind::OpenCode, rows)
    }

    /// A v1 `message` row with its parts, each `(id, data)`.
    fn v1(id: &str, role: &str, parts: &[(&str, Value)]) -> Value {
        let parts: Vec<Value> = (parts.iter())
            .map(|(pid, data)| json!({ "id": pid, "message_id": id, "data": data }))
            .collect();
        json!({ "id": id, "session_id": "ses_1", "time_created": 1000,
            "data": { "role": role, "time": { "created": 1000 } }, "parts": parts })
    }

    /// A v2 `session_message` row.
    fn v2(id: &str, kind: &str, data: Value) -> Value {
        json!({ "id": id, "session_id": "ses_1", "type": kind, "seq": 1, "time_created": 2000, "data": data })
    }

    fn line(side: DiffSide, text: &str) -> DiffLine {
        DiffLine {
            side,
            text: text.into(),
        }
    }

    #[test]
    fn v1_user_answer_and_thinking() {
        let items = read(&[
            v1(
                "m1",
                "user",
                &[
                    ("p1", json!({ "type": "text", "text": "fix it" })),
                    (
                        "p2",
                        json!({ "type": "text", "text": "injected", "synthetic": true }),
                    ),
                ],
            ),
            v1(
                "m2",
                "assistant",
                &[
                    ("p3", json!({ "type": "step-start" })),
                    ("p4", json!({ "type": "reasoning", "text": "Hmm." })),
                    ("p5", json!({ "type": "text", "text": "One." })),
                    ("p6", json!({ "type": "text", "text": "Two." })),
                    ("p7", json!({ "type": "step-finish", "reason": "stop" })),
                ],
            ),
        ]);
        let want = vec![
            Item::User {
                id: "p1".into(),
                at: Some(1000),
                text: "fix it".into(),
            },
            Item::Thinking {
                id: "p4".into(),
                text: "Hmm.".into(),
            },
            Item::Assistant {
                id: "p5".into(),
                at: Some(1000),
                blocks: md_blocks::blocks("One.\n\nTwo.").into(),
            },
        ];
        assert_eq!(items, want);
    }

    #[test]
    fn v2_user_answer_and_thinking() {
        let items = read(&[
            v2(
                "s0",
                "system",
                json!({ "text": "You are", "time": { "created": 1 } }),
            ),
            v2(
                "s1",
                "user",
                json!({ "text": "hello", "time": { "created": 5 } }),
            ),
            v2(
                "s2",
                "synthetic",
                json!({ "text": "<image>", "time": { "created": 6 } }),
            ),
            v2(
                "s3",
                "assistant",
                json!({ "time": { "created": 7 }, "content": [
                    { "type": "reasoning", "text": "Think." },
                    { "type": "text", "text": "Hi." },
                ]}),
            ),
            v2("s4", "idle", json!({ "outcome": "succeeded" })),
            v2(
                "s5",
                "model-switched",
                json!({ "model": { "id": "big-model" } }),
            ),
        ]);
        let want = vec![
            Item::User {
                id: "s1".into(),
                at: Some(5),
                text: "hello".into(),
            },
            Item::Thinking {
                id: "s3.0".into(),
                text: "Think.".into(),
            },
            Item::Assistant {
                id: "s3.1".into(),
                at: Some(7),
                blocks: md_blocks::blocks("Hi.").into(),
            },
            Item::Note {
                id: "s5".into(),
                text: "Set model to big-model".into(),
            },
        ];
        assert_eq!(items, want);
    }

    #[test]
    fn v1_tools_say_status_and_time() {
        let tool = |status: &str, extra: Value| {
            let mut state = json!({ "status": status, "input": { "filePath": "/r/a.rs", "limit": 5 },
                "time": { "start": 100, "end": 350 } });
            state
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            json!({ "type": "tool", "tool": "read", "callID": format!("call_{status}"), "state": state })
        };
        let items = read(&[v1(
            "m1",
            "assistant",
            &[
                ("p1", tool("completed", json!({ "output": "fn a()" }))),
                ("p2", tool("error", json!({ "error": "No such file" }))),
                ("p3", tool("running", json!({}))),
            ],
        )]);
        let out = |i: usize| match &items[i] {
            Item::Tool {
                id,
                name,
                gist,
                output,
                ..
            } => (id.as_str(), name.as_str(), gist.as_str(), output.clone()),
            other => panic!("{other:?}"),
        };
        let done = ToolOut {
            text: "fn a()".into(),
            failed: false,
            ms: Some(250),
        };
        assert_eq!(out(0), ("call_completed", "read", "/r/a.rs", Some(done)));
        let failed = ToolOut {
            text: "No such file".into(),
            failed: true,
            ms: Some(250),
        };
        assert_eq!(out(1).3, Some(failed));
        assert_eq!(out(2).3, None);
    }

    #[test]
    fn v2_tools_say_status_and_time() {
        let items = read(&[v2(
            "s1",
            "assistant",
            json!({ "content": [
                { "type": "tool", "id": "t1", "name": "bash",
                  "state": { "status": "completed", "input": { "command": "ls -la", "workdir": "/r" },
                    "content": [{ "type": "text", "text": "a\nb" }] },
                  "time": { "created": 10, "ran": 20, "completed": 70 } },
                { "type": "tool", "id": "t2", "name": "glob",
                  "state": { "status": "error", "input": { "path": "/r", "pattern": "*.rs" },
                    "error": { "type": "unknown", "message": "Denied" } },
                  "time": { "created": 10, "completed": 15 } },
            ]}),
        )]);
        let Item::Tool {
            name,
            gist,
            input,
            output,
            ..
        } = &items[0]
        else {
            panic!("{items:?}")
        };
        assert_eq!(
            (name.as_str(), gist.as_str(), input.as_str()),
            ("bash", "ls -la", "ls -la")
        );
        assert_eq!(
            output,
            &Some(ToolOut {
                text: "a\nb".into(),
                failed: false,
                ms: Some(50)
            })
        );
        let Item::Tool { gist, output, .. } = &items[1] else {
            panic!()
        };
        assert_eq!(gist, "*.rs");
        assert_eq!(
            output,
            &Some(ToolOut {
                text: "Denied".into(),
                failed: true,
                ms: Some(5)
            })
        );
    }

    #[test]
    fn edit_write_and_patch_are_edits() {
        use DiffSide::{Added, Context, Removed};
        let done = json!({ "status": "completed", "output": "ok", "metadata": {}, "time": { "start": 1, "end": 2 } });
        let with = |input: Value| {
            let mut state = done.clone();
            state["input"] = input;
            state
        };
        let unified =
            "Index: /r/c.rs\n===\n--- /r/c.rs\n+++ /r/c.rs\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n";
        let mut patched = with(json!({ "patchText": "*** Begin Patch\n*** End Patch" }));
        patched["metadata"]["files"] = json!([
            { "filePath": "/r/c.rs", "relativePath": "c.rs", "type": "update", "patch": unified },
        ]);
        let asked = "*** Begin Patch\n*** Add File: d.rs\n+one\n*** Update File: e.rs\n@@\n-x\n+y\n*** End Patch";
        let items = read(&[
            v1(
                "m1",
                "assistant",
                &[
                    (
                        "p1",
                        json!({ "type": "tool", "tool": "edit", "callID": "c1",
                    "state": with(json!({ "filePath": "/r/a.rs", "oldString": "a", "newString": "b" })) }),
                    ),
                    (
                        "p2",
                        json!({ "type": "tool", "tool": "write", "callID": "c2",
                    "state": with(json!({ "filePath": "/r/b.rs", "content": "x\ny" })) }),
                    ),
                    (
                        "p3",
                        json!({ "type": "tool", "tool": "apply_patch", "callID": "c3", "state": patched }),
                    ),
                ],
            ),
            v2(
                "s1",
                "assistant",
                json!({ "content": [
                    { "type": "tool", "id": "c4", "name": "patch",
                      "state": { "status": "pending", "input": { "patchText": asked } } },
                    { "type": "tool", "id": "c5", "name": "write",
                      "state": { "status": "running", "input": { "path": "/r/f.rs", "content": "z" } } },
                ]}),
            ),
        ]);
        let edits: Vec<(&str, &str, Vec<DiffLine>, bool)> = (items.iter())
            .map(|item| match item {
                Item::Edit {
                    id,
                    path,
                    diff,
                    output,
                } => (id.as_str(), path.as_str(), diff.clone(), output.is_some()),
                other => panic!("{other:?}"),
            })
            .collect();
        let want: Vec<(&str, &str, Vec<DiffLine>, bool)> = vec![
            (
                "c1",
                "/r/a.rs",
                vec![line(Removed, "a"), line(Added, "b")],
                true,
            ),
            (
                "c2",
                "/r/b.rs",
                vec![line(Added, "x"), line(Added, "y")],
                true,
            ),
            (
                "c3",
                "/r/c.rs",
                vec![
                    line(Context, "keep"),
                    line(Removed, "old"),
                    line(Added, "new"),
                ],
                true,
            ),
            ("c4", "d.rs", vec![line(Added, "one")], false),
            (
                "c4.1",
                "e.rs",
                vec![line(Removed, "x"), line(Added, "y")],
                false,
            ),
            ("c5", "/r/f.rs", vec![line(Added, "z")], false),
        ];
        assert_eq!(edits, want);
    }

    #[test]
    fn errors_and_compaction_are_notes() {
        let mut failed = v1(
            "m1",
            "assistant",
            &[("p1", json!({ "type": "text", "text": "Partly." }))],
        );
        failed["data"]["error"] =
            json!({ "name": "APIError", "data": { "message": "Rate limited" } });
        let mut stopped = v1("m2", "assistant", &[]);
        stopped["data"]["error"] =
            json!({ "name": "MessageAbortedError", "data": { "message": "Aborted" } });
        let mut summary = v1(
            "m4",
            "assistant",
            &[("p3", json!({ "type": "text", "text": "Summary of all" }))],
        );
        summary["data"]["summary"] = true.into();
        let items = read(&[
            failed,
            stopped,
            v1(
                "m3",
                "user",
                &[("p2", json!({ "type": "compaction", "auto": true }))],
            ),
            summary,
            v2(
                "s1",
                "compaction",
                json!({ "status": "completed", "summary": "Long story" }),
            ),
            v2(
                "s2",
                "assistant",
                json!({ "content": [], "error": { "type": "provider.error", "message": "Overloaded" } }),
            ),
            v2(
                "s3",
                "assistant",
                json!({ "content": [], "error": { "type": "aborted", "message": "Aborted" } }),
            ),
        ]);
        let notes: Vec<(&str, &str)> = (items.iter().skip(1))
            .map(|item| match item {
                Item::Note { id, text } => (id.as_str(), text.as_str()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            notes,
            [
                ("m1.error", "Rate limited"),
                ("m2.error", "Interrupted by the user"),
                ("p2", "Conversation compacted"),
                ("s1", "Conversation compacted"),
                ("s2.error", "Overloaded"),
                ("s3.error", "Interrupted by the user"),
            ]
        );
    }

    #[test]
    fn reading_again_gives_the_same_items() {
        let rows = [
            v1(
                "m1",
                "user",
                &[("p1", json!({ "type": "text", "text": "go" }))],
            ),
            v2(
                "s1",
                "assistant",
                json!({ "content": [
                    { "type": "text", "text": "Ok." },
                    { "type": "tool", "id": "t1", "name": "todowrite",
                      "state": { "status": "completed", "input": { "todos": [] }, "content": [] } },
                ]}),
            ),
        ];
        let first = read(&rows);
        assert_eq!(first, read(&rows));
        let ids: Vec<&str> = first.iter().map(Item::id).collect();
        assert_eq!(ids, ["p1", "s1.0", "t1"]);
    }
}
