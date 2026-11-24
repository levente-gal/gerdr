//! pi's session (`~/.pi/agent/sessions/--<folder>--/<time>_<id>.jsonl`): a
//! `session` header line, then a line per entry, each with its `id`.
//! `message` entries hold what was said: the user's, the assistant's
//! (text, thinking and tool calls as parts), a tool's result, a shell
//! command the user ran (`!`).
//!
//! The entries are a tree: `/tree` goes back to an earlier entry and the
//! chat goes on from there. An entry whose parent isn't the entry before it
//! begins such a branch: what came after its parent goes, and the chat
//! reads on from there, as pi shows it.

use serde_json::Value;

use super::{
    Item, Reader, ToolOut, cap, change,
    opencode::{capped, gist, input_text},
};

/// Said where the chat went back to an earlier point and on from there.
const BACK: &str = "Went back to an earlier point of the chat";

impl Reader {
    /// Read session line `line`.
    pub(super) fn pi_line(&mut self, line: &Value) {
        let id = (line["id"].as_str()).map_or_else(|| format!("line{}", self.lines), str::to_owned);
        let kind = line["type"].as_str().unwrap_or_default();
        if let Some(parent) = line["parentId"].as_str()
            && self
                .last_entry
                .as_deref()
                .is_some_and(|last| last != parent)
            && let Some(&kept) = self.entries.get(parent)
        {
            self.back_to(kept);
            // A summary of the branch left says so itself.
            if kind != "branch_summary" {
                let (id, text) = (format!("{id}.back"), BACK.to_owned());
                self.push(Item::Note { id, text });
            }
        }
        self.pi_entry(line, kind, id.clone());
        self.entries.insert(id.clone(), self.items.len());
        self.last_entry = Some(id);
    }

    /// The chat back to its first `kept` items: a branch of it left.
    fn back_to(&mut self, kept: usize) {
        self.items.truncate(kept);
        self.calls.retain(|_, (at, _)| *at < kept);
        self.answer = None;
    }

    fn pi_entry(&mut self, line: &Value, kind: &str, id: String) {
        match kind {
            "message" => self.pi_message(&line["message"], id),
            "compaction" => {
                let text = "The chat was compacted".to_owned();
                self.push(Item::Note { id, text });
            }
            "branch_summary" => {
                let text = BACK.to_owned();
                self.push(Item::Note { id, text });
            }
            // The header, model and thinking level changes, labels, names,
            // extensions' own entries and context edits.
            _ => {}
        }
    }

    fn pi_message(&mut self, m: &Value, id: String) {
        let at = m["timestamp"].as_u64();
        match m["role"].as_str().unwrap_or_default() {
            "user" => {
                let text = texts(&m["content"]);
                let text = text.trim();
                if !text.is_empty() {
                    let text = text.to_owned();
                    self.push(Item::User { id, at, text });
                }
            }
            "assistant" => {
                let parts = m["content"].as_array().into_iter().flatten();
                for (k, part) in parts.enumerate() {
                    let part_id = format!("{id}.{k}");
                    match part["type"].as_str().unwrap_or_default() {
                        "text" => {
                            let text = part["text"].as_str().unwrap_or_default();
                            self.answer(&id, part_id, at, text);
                        }
                        "thinking" => {
                            let text = part["thinking"].as_str().unwrap_or_default().trim();
                            if !text.is_empty() {
                                let text = text.to_owned();
                                self.push(Item::Thinking { id: part_id, text });
                            }
                        }
                        "toolCall" => self.pi_call(part, part_id, at),
                        _ => {}
                    }
                }
                let said = match m["stopReason"].as_str() {
                    Some("error") => m["errorMessage"].as_str().map(str::trim),
                    Some("aborted") => Some("Interrupted by the user"),
                    _ => None,
                };
                if let Some(text) = said.filter(|t| !t.is_empty()) {
                    let (id, text) = (format!("{id}.end"), text.to_owned());
                    self.push(Item::Note { id, text });
                }
            }
            "toolResult" => {
                let call = m["toolCallId"].as_str().unwrap_or_default();
                let out = ToolOut {
                    text: cap(texts(&m["content"]).trim_end()),
                    failed: m["isError"] == true,
                    ms: m["durationMs"].as_u64(),
                };
                self.fill(call, out);
            }
            // A command the user ran with `!`.
            "bashExecution" => {
                let command = m["command"].as_str().unwrap_or_default();
                let exit = m["exitCode"].as_i64();
                let output = ToolOut {
                    text: cap(m["output"].as_str().unwrap_or_default().trim_end()),
                    failed: m["cancelled"] == true || exit.is_some_and(|code| code != 0),
                    ms: None,
                };
                self.push(Item::Tool {
                    id,
                    name: "bash".to_owned(),
                    gist: command.lines().next().unwrap_or_default().to_owned(),
                    input: command.to_owned(),
                    output: Some(output),
                });
            }
            // Its instructions, and extensions' messages.
            _ => {}
        }
    }

    /// Tool call `part`: an edit or a write with its change, else a tool
    /// with its gist.
    fn pi_call(&mut self, part: &Value, id: String, at: Option<u64>) {
        let call = part["id"]
            .as_str()
            .map_or_else(|| id.clone(), str::to_owned);
        let name = part["name"].as_str().unwrap_or("tool");
        let args = &part["arguments"];
        let said = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_owned();
        let diff = match name {
            // `edits: [{oldText, newText}]`; older sessions' one edit is
            // at the top.
            "edit" => Some(match args["edits"].as_array() {
                Some(edits) => (edits.iter())
                    .flat_map(|e| change(&said(e, "oldText"), &said(e, "newText")))
                    .collect(),
                None => change(&said(args, "oldText"), &said(args, "newText")),
            }),
            "write" => Some(change("", &said(args, "content"))),
            _ => None,
        };
        let item = match (args["path"].as_str(), diff) {
            (Some(path), Some(diff)) => Item::Edit {
                id,
                path: path.to_owned(),
                diff: capped(diff),
                output: None,
            },
            _ => Item::Tool {
                id,
                name: name.to_owned(),
                gist: gist(args),
                input: input_text(args),
                output: None,
            },
        };
        self.push_call(call, at, item);
    }
}

/// The text of `content`: a string, or its text parts (an image said as
/// one).
fn texts(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => (parts.iter())
            .filter_map(|p| match p["type"].as_str() {
                Some("image") => Some("[image]"),
                _ => p["text"].as_str(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{ai_work::AgentKind, chat::DiffSide};

    fn read(lines: &[Value]) -> Vec<Item> {
        let mut reader = Reader::new(AgentKind::Pi);
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend(line.to_string().bytes());
            bytes.push(b'\n');
        }
        reader.feed(&bytes);
        reader.items
    }

    fn message(id: &str, message: Value) -> Value {
        json!({"type": "message", "id": id, "parentId": null,
               "timestamp": "2026-01-02T03:04:05.000Z", "message": message})
    }

    #[test]
    fn a_turn_reads_as_words_thinking_tools_and_results() {
        let items = read(&[
            json!({"type": "session", "version": 3, "id": "s1",
                   "timestamp": "2026-01-02T03:04:00.000Z", "cwd": "/w"}),
            message("e0", json!({"role": "system", "content": "You are pi"})),
            message(
                "e1",
                json!({"role": "user", "timestamp": 1000,
                                 "content": [{"type": "text", "text": "fix it"}]}),
            ),
            message(
                "e2",
                json!({"role": "assistant", "timestamp": 2000, "stopReason": "toolUse",
                "content": [
                    {"type": "thinking", "thinking": "Looking"},
                    {"type": "text", "text": "Running the tests."},
                    {"type": "toolCall", "id": "c1", "name": "bash",
                     "arguments": {"command": "cargo test", "timeout": 60}},
                    {"type": "toolCall", "id": "c2", "name": "edit",
                     "arguments": {"path": "src/a.rs", "edits": [{"oldText": "a", "newText": "b"}]}},
                ]}),
            ),
            message(
                "e3",
                json!({"role": "toolResult", "toolCallId": "c1", "toolName": "bash",
                "content": [{"type": "text", "text": "1 failed"}], "isError": true, "durationMs": 1500}),
            ),
            message(
                "e4",
                json!({"role": "toolResult", "toolCallId": "c2", "toolName": "edit",
                "content": [{"type": "text", "text": "ok"}], "isError": false, "durationMs": 3}),
            ),
            message(
                "e5",
                json!({"role": "assistant", "stopReason": "error",
                "errorMessage": "rate limited", "content": []}),
            ),
        ]);
        assert!(
            matches!(&items[0], Item::User { id, at: Some(1000), text } if id == "e1" && text == "fix it")
        );
        assert!(matches!(&items[1], Item::Thinking { text, .. } if text == "Looking"));
        assert!(matches!(&items[2], Item::Assistant { .. }));
        let Item::Tool {
            gist,
            output: Some(out),
            ..
        } = &items[3]
        else {
            panic!("{:?}", items[3])
        };
        assert_eq!(
            (gist.as_str(), out.failed, out.ms),
            ("cargo test", true, Some(1500))
        );
        let Item::Edit {
            path,
            diff,
            output: Some(out),
            ..
        } = &items[4]
        else {
            panic!("{:?}", items[4])
        };
        assert_eq!(
            (path.as_str(), diff.len(), out.ms),
            ("src/a.rs", 2, Some(3))
        );
        assert_eq!(diff[1].side, DiffSide::Added);
        assert!(matches!(&items[5], Item::Note { text, .. } if text == "rate limited"));
        assert_eq!(items.len(), 6);
    }

    #[test]
    fn a_branch_shows_only_from_where_it_went_back() {
        let entry = |id: &str, parent: Option<&str>, text: &str| {
            let mut m = message(id, json!({"role": "user", "content": text}));
            m["parentId"] = json!(parent);
            m
        };
        let items = read(&[
            entry("e1", None, "one"),
            entry("e2", Some("e1"), "two"),
            entry("e3", Some("e2"), "three"),
            // `/tree` back to e1, then on.
            entry("e4", Some("e1"), "four"),
            entry("e5", Some("e4"), "five"),
        ]);
        let said: Vec<&str> = (items.iter())
            .map(|item| match item {
                Item::User { text, .. } => text.as_str(),
                Item::Note { .. } => "back",
                _ => "?",
            })
            .collect();
        assert_eq!(said, ["one", "back", "four", "five"]);
        // A parent read before reading began: nothing to go back to.
        let items = read(&[
            entry("e8", Some("e7"), "eight"),
            entry("e9", Some("e2"), "nine"),
        ]);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn a_command_the_user_ran_and_a_compaction_show() {
        let items = read(&[
            message(
                "e1",
                json!({"role": "bashExecution", "command": "ls", "output": "a\n",
                                 "exitCode": 0, "cancelled": false, "truncated": false}),
            ),
            json!({"type": "compaction", "id": "e2", "summary": "...", "firstKeptEntryId": "e1"}),
            message(
                "e3",
                json!({"role": "assistant", "content": [
                {"type": "toolCall", "id": "c1", "name": "edit",
                 "arguments": {"path": "a.rs", "oldText": "x", "newText": "y"}}]}),
            ),
        ]);
        assert!(
            matches!(&items[0], Item::Tool { gist, output: Some(out), .. } if gist == "ls" && !out.failed)
        );
        assert!(matches!(&items[1], Item::Note { .. }));
        assert!(
            matches!(&items[2], Item::Edit { diff, .. } if diff.len() == 2),
            "an older one edit"
        );
    }
}
