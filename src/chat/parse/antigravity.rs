//! The Antigravity CLI's transcript of a conversation
//! (`~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/
//! transcript_full.jsonl`): a line per step, `{step_index, source, type,
//! status, created_at, ...}`. The user's input (`USER_INPUT`), the model's
//! turns (`PLANNER_RESPONSE`: thinking, its words, tool calls by name and
//! arguments only), each call's result as a step of its own after it
//! (`GENERIC`), in the order of the calls, and errors.
//!
//! shortcut: a step's line is read once, as first written; read a step
//! again if Antigravity turns out to write one line per state of a step.

use serde_json::Value;

use super::{
    Item, Reader, ToolOut, cap, change,
    opencode::{capped, input_text},
};
use crate::{ai_work::usage::iso_ms, stats_panel::tool_gist};

/// What a tool's arguments are named, its gist first.
const GIST_KEYS: [&str; 9] = [
    "CommandLine",
    "AbsolutePath",
    "Path",
    "TargetFile",
    "DirectoryPath",
    "SearchPath",
    "Query",
    "Url",
    "toolSummary",
];

impl Reader {
    /// Read transcript line `line`.
    pub(super) fn antigravity_line(&mut self, line: &Value) {
        let Some(step) = line["step_index"].as_u64() else {
            return;
        };
        if !self.steps.insert(step) {
            return;
        }
        let id = format!("step{step}");
        let at = line["created_at"].as_str().and_then(iso_ms);
        let said = |key: &str| line[key].as_str().unwrap_or_default().trim();
        match line["type"].as_str().unwrap_or_default() {
            "USER_INPUT" => {
                let text = request(said("content"));
                if !text.is_empty() {
                    let text = text.to_owned();
                    self.push(Item::User { id, at, text });
                }
            }
            "PLANNER_RESPONSE" => {
                if !said("thinking").is_empty() {
                    let (id, text) = (format!("{id}.t"), said("thinking").to_owned());
                    self.push(Item::Thinking { id, text });
                }
                self.answer(&id, format!("{id}.a"), at, said("content"));
                let calls = line["tool_calls"].as_array().into_iter().flatten();
                for (k, call) in calls.enumerate() {
                    self.antigravity_call(call, format!("{id}.{k}"), at);
                }
            }
            // A call's result: the first call still waiting gets it.
            "GENERIC" if line["source"] == "MODEL" => {
                if let Some(call) = self.waiting.pop_front() {
                    self.fill(&call, outcome(said("content")));
                }
            }
            "ERROR_MESSAGE" => {
                let text = said("error").lines().next().unwrap_or_default().to_owned();
                if !text.is_empty() {
                    self.push(Item::Note { id, text });
                }
            }
            // Antigravity's own messages to the model.
            _ => {}
        }
    }

    /// Tool call `call` (`{name, args}`): a file written or replaced in with
    /// its change, else a tool with its gist.
    fn antigravity_call(&mut self, call: &Value, id: String, at: Option<u64>) {
        let name = call["name"].as_str().unwrap_or("tool");
        let args = &call["args"];
        let said = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_owned();
        let replaced = |chunk: &Value| {
            change(
                &said(chunk, "TargetContent"),
                &said(chunk, "ReplacementContent"),
            )
        };
        let diff = match name {
            "write_to_file" => Some(change("", &said(args, "CodeContent"))),
            "replace_file_content" | "multi_replace_file_content" => {
                Some(match args["ReplacementChunks"].as_array() {
                    Some(chunks) => chunks.iter().flat_map(replaced).collect(),
                    None => replaced(args),
                })
            }
            _ => None,
        };
        let item = match (args["TargetFile"].as_str(), diff) {
            (Some(path), Some(diff)) => Item::Edit {
                id: id.clone(),
                path: path.to_owned(),
                diff: capped(diff),
                output: None,
            },
            _ => Item::Tool {
                id: id.clone(),
                name: name.to_owned(),
                gist: (GIST_KEYS.iter())
                    .find_map(|k| args.get(*k).and_then(tool_gist))
                    .unwrap_or_default(),
                input: match args["CommandLine"].as_str() {
                    Some(command) => command.to_owned(),
                    None => input_text(args),
                },
                output: None,
            },
        };
        self.waiting.push_back(id.clone());
        self.push_call(id, at, item);
    }
}

/// What the user typed of input `content`: inside its `<USER_REQUEST>`,
/// without what Antigravity adds around it.
fn request(content: &str) -> &str {
    let inside = (content.split_once("<USER_REQUEST>"))
        .and_then(|(_, rest)| rest.split_once("</USER_REQUEST>"))
        .map_or(content, |(said, _)| said);
    inside.trim()
}

/// A result step's `content`: `Created At:` and `Completed At:` lines (how
/// long it took), then what it gave; a command that exited other than 0
/// failed.
fn outcome(content: &str) -> ToolOut {
    let content = content.replace('\r', "");
    let mut lines = content.lines().peekable();
    let (mut created, mut completed) = (None, None);
    while let Some(line) = lines.peek() {
        let time = |prefix: &str| line.strip_prefix(prefix).and_then(|t| iso_ms(t.trim()));
        match (time("Created At:"), time("Completed At:")) {
            (Some(at), _) => created = created.or(Some(at)),
            (_, Some(at)) => completed = completed.or(Some(at)),
            _ => break,
        }
        lines.next();
    }
    let text = lines.collect::<Vec<_>>().join("\n");
    let exit = (text.split_once("exit code ")).and_then(|(_, rest)| {
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse::<i64>()
            .ok()
    });
    ToolOut {
        failed: exit.is_some_and(|code| code != 0),
        ms: completed
            .zip(created)
            .and_then(|(end, start)| end.checked_sub(start)),
        text: cap(text.trim_end()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ai_work::AgentKind;

    fn read(lines: &[Value]) -> Vec<Item> {
        let mut reader = Reader::new(AgentKind::Antigravity);
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend(line.to_string().bytes());
            bytes.push(b'\n');
        }
        reader.feed(&bytes);
        reader.items
    }

    fn step(n: u64, kind: &str, source: &str, rest: Value) -> Value {
        let mut line = json!({"step_index": n, "source": source, "type": kind, "status": "DONE",
                              "created_at": "2026-01-02T03:04:05Z"});
        line.as_object_mut()
            .unwrap()
            .extend(rest.as_object().unwrap().clone());
        line
    }

    fn result(n: u64, body: &str) -> Value {
        let content = format!(
            "Created At: 2026-01-02T05:04:05+02:00\nCompleted At: 2026-01-02T05:04:07+02:00\n{body}"
        );
        step(n, "GENERIC", "MODEL", json!({"content": content}))
    }

    #[test]
    fn calls_get_their_results_in_order() {
        let items = read(&[
            step(
                0,
                "USER_INPUT",
                "USER_EXPLICIT",
                json!({"content":
                "<USER_REQUEST>\nlist files\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nnow\n</ADDITIONAL_METADATA>"}),
            ),
            step(
                1,
                "ERROR_MESSAGE",
                "SYSTEM",
                json!({"error": "API error (attempt 1): 503\nmore"}),
            ),
            step(
                2,
                "PLANNER_RESPONSE",
                "MODEL",
                json!({"thinking": "Look", "tool_calls": [
                    {"name": "read_file", "args": {"Path": "/w/a.md"}},
                    {"name": "run_command", "args": {"CommandLine": "ls -la", "WaitMsBeforeAsync": 500}},
                ]}),
            ),
            result(3, "Showing lines 1-2 of 2\n1: a\n2: b"),
            result(
                4,
                "Command completed with exit code 2.\r\n\r\nOutput:\r\nno",
            ),
            step(5, "SYSTEM_MESSAGE", "SYSTEM", json!({"content": "nudge"})),
            step(
                6,
                "PLANNER_RESPONSE",
                "MODEL",
                json!({"content": "There are **2** files."}),
            ),
            // The same step again: read once.
            step(
                6,
                "PLANNER_RESPONSE",
                "MODEL",
                json!({"content": "There are **2** files."}),
            ),
        ]);
        assert!(matches!(&items[0], Item::User { text, at: Some(_), .. } if text == "list files"));
        assert!(
            matches!(&items[1], Item::Note { text, .. } if text == "API error (attempt 1): 503")
        );
        assert!(matches!(&items[2], Item::Thinking { text, .. } if text == "Look"));
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
            ("/w/a.md", false, Some(2000))
        );
        assert!(out.text.starts_with("Showing lines"));
        let Item::Tool {
            gist,
            input,
            output: Some(out),
            ..
        } = &items[4]
        else {
            panic!("{:?}", items[4])
        };
        assert_eq!(
            (gist.as_str(), input.as_str(), out.failed),
            ("ls -la", "ls -la", true)
        );
        assert!(!out.text.contains('\r'));
        assert!(matches!(&items[5], Item::Assistant { .. }));
        assert_eq!(items.len(), 6);
    }

    #[test]
    fn a_file_written_or_replaced_in_shows_its_change() {
        let items = read(&[step(
            1,
            "PLANNER_RESPONSE",
            "MODEL",
            json!({"tool_calls": [
                {"name": "write_to_file", "args": {"TargetFile": "/w/n.rs", "CodeContent": "a\nb"}},
                {"name": "replace_file_content", "args": {"TargetFile": "/w/o.rs", "ReplacementChunks": [
                    {"TargetContent": "x", "ReplacementContent": "y"}]}},
            ]}),
        )]);
        assert!(
            matches!(&items[0], Item::Edit { path, diff, .. } if path == "/w/n.rs" && diff.len() == 2)
        );
        assert!(
            matches!(&items[1], Item::Edit { path, diff, .. } if path == "/w/o.rs" && diff.len() == 2)
        );
    }
}
