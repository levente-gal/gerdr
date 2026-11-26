//! Claude Code's transcript: a JSON line per user turn, per block of an
//! assistant message (its lines share `message.id`), per tool result, plus
//! lines of its own bookkeeping. Subagents' lines (`isSidechain`) go in a
//! `Subagent` under the `Task`/`Agent` call that started them.

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde_json::Value;

use crate::{
    ai_work::{AgentKind, usage::iso_ms},
    chat::{Item, ToolOut, parse::Reader},
    feature::verify::strip_escapes,
    integrations::{
        Parser,
        common::build::{
            Builder, INTERRUPTED, Sub, cant_show, cap, change, edit_or_tool, flat_edits,
            is_plan_file, tag, texts,
        },
    },
    stats_panel::tool_gist,
};

/// What a reader of Claude Code's keeps of its own.
#[derive(Default)]
pub(crate) struct Claude {
    /// `Task`/`Agent` calls whose work hasn't shown yet, by their id: the
    /// prompt they gave and their title.
    tasks: HashMap<String, (String, String)>,
    /// A subagent's call by its agent id, as its result says.
    agent_calls: HashMap<String, String>,
    /// The model the chat is on, as its lines last said.
    model: Option<String>,
}

impl Parser for Claude {
    /// `line` read here, or in its subagent's reader.
    fn line(&mut self, b: &mut Builder, line: &Value) {
        if line["isSidechain"] != true {
            if let Some(model) = of_line(line) {
                self.model = Some(model);
            }
            return self.sub_line(b, line);
        }
        let agent = line["agentId"].as_str();
        let known = (agent.and_then(|a| b.sub_of.get(a)))
            .or_else(|| line["parentUuid"].as_str().and_then(|p| b.sub_of.get(p)))
            .copied();
        // A subagent whose call can't be told is left out.
        let Some(sub) = known.or_else(|| self.new_sub(b, line, agent)) else {
            return;
        };
        if let (None, Some(uuid)) = (agent, line["uuid"].as_str()) {
            b.sub_of.insert(uuid.to_owned(), sub);
        }
        let sub = &mut b.subs[sub];
        let reader = &mut sub.reader;
        reader.b.lines = b.lines;
        reader.p.sub_line(&mut reader.b, line);
        sub.changed = true;
        if let Some(at) = line["timestamp"].as_str().and_then(iso_ms) {
            sub.span = Some(
                sub.span
                    .map_or((at, at), |(from, to)| (from.min(at), to.max(at))),
            );
        }
    }

    fn sub_line(&mut self, b: &mut Builder, line: &Value) {
        let at = line["timestamp"].as_str().and_then(iso_ms);
        match line["type"].as_str().unwrap_or_default() {
            "user" => self.user(b, line, at),
            "assistant" => self.assistant(b, line, at),
            "system" if line["isMeta"] != true => {
                if let Some(text) = line["content"].as_str() {
                    let id = id(b, line, 0);
                    b.push_some(told(text, id, at, false));
                }
            }
            // A prompt typed while the agent worked.
            "attachment" => {
                let queued = &line["attachment"];
                if queued["type"] == "queued_command"
                    && queued["commandMode"] == "prompt"
                    && queued["isMeta"] != true
                    && let Some(text) = queued["prompt"].as_str()
                {
                    let id = id(b, line, 0);
                    b.push_some(told(text, id, at, true));
                }
            }
            // The API request again, as sent.
            "api-request-blob" => {}
            // What has no message is Claude Code's bookkeeping: titles,
            // modes, file snapshots, costs.
            kind if line.get("message").is_some() => {
                let id = id(b, line, 0);
                let text = cant_show(kind);
                b.push(Item::Note { id, text });
            }
            _ => {}
        }
    }

    /// Whole lines of subagent `agent`'s own transcript: Claude Code keeps
    /// each in `<chat>/subagents/agent-<id>.jsonl`, apart from the chat's.
    /// `call` is the call that started it, as the file's meta says; its
    /// work goes under it once that call is read.
    fn subagent(&mut self, b: &mut Builder, agent: &str, call: &str, lines: &[u8]) {
        (self.agent_calls).insert(agent.to_owned(), call.to_owned());
        for raw in lines.split(|b| *b == b'\n') {
            // A first line cut short, where reading began mid-file.
            let Ok(mut line) = serde_json::from_slice::<Value>(raw) else {
                continue;
            };
            line["isSidechain"] = true.into();
            line["agentId"] = agent.into();
            self.line(b, &line);
        }
    }

    fn model(&self) -> Option<String> {
        self.model.clone()
    }
}

impl Claude {
    /// A subagent for sidechain `line`, when the call that started it is
    /// known: by its agent id, or by its first line being the call's prompt.
    fn new_sub(&mut self, b: &mut Builder, line: &Value, agent: Option<&str>) -> Option<usize> {
        let by_agent = agent.and_then(|a| self.agent_calls.get(a)).cloned();
        let call = by_agent.or_else(|| {
            let prompt = (line["type"] == "user").then(|| texts(&line["message"]["content"]))?;
            let prompt = prompt.trim();
            let found = self.tasks.iter().find(|(_, (p, _))| p.trim() == prompt);
            found.map(|(id, _)| id.clone())
        })?;
        let (_, title) = self.tasks.remove(&call)?;
        // In the place of its call's card, with its result.
        let at = b.calls.get(&call).map(|&(at, _)| at)?;
        let Item::Tool { output, .. } = &b.items[at] else {
            return None;
        };
        let output = output.clone();
        b.subs.push(Sub::new(at, Reader::new(AgentKind::Claude)));
        let id = format!("agent:{call}");
        b.items[at] = Item::Subagent {
            id,
            title,
            items: Arc::default(),
            output,
        };
        let sub = b.subs.len() - 1;
        if let Some(agent) = agent {
            b.sub_of.insert(agent.to_owned(), sub);
        }
        Some(sub)
    }

    fn user(&mut self, b: &mut Builder, line: &Value, at: Option<u64>) {
        let content = &line["message"]["content"];
        for part in content.as_array().into_iter().flatten() {
            if part["type"] == "tool_result" {
                self.result(b, part, line, at);
            }
        }
        // Meta lines are what Claude Code tells the model; compaction
        // summaries too (the "compacted" system line notes it).
        if line["isMeta"] == true || line["isCompactSummary"] == true {
            return;
        }
        let id = id(b, line, 0);
        b.push_some(told(&texts(content), id, at, true));
    }

    /// Tool result `part` of user `line`, given to its call's item.
    fn result(&mut self, b: &mut Builder, part: &Value, line: &Value, at: Option<u64>) {
        let Some(call) = part["tool_use_id"].as_str() else {
            return;
        };
        // A call before where reading started has nothing to fill.
        let Some(&(_, called)) = b.calls.get(call) else {
            return;
        };
        let done = &line["toolUseResult"];
        let ms = (done["totalDurationMs"].as_u64())
            .or(done["durationMs"].as_u64())
            .or_else(|| {
                let secs = done["durationSeconds"].as_f64()?;
                Duration::try_from_secs_f64(secs)
                    .ok()
                    .map(|d| d.as_millis() as u64)
            })
            .or_else(|| at?.checked_sub(called?))
            // Launched in the background: how long launching took says
            // nothing of the agent's work.
            .filter(|_| done["status"] != "async_launched");
        let out = ToolOut {
            text: cap(&texts(&part["content"])),
            failed: part["is_error"] == true,
            ms,
        };
        b.fill(call, out);
        if let Some(agent) = done["agentId"].as_str() {
            self.agent_calls.insert(agent.to_owned(), call.to_owned());
        }
    }

    fn assistant(&mut self, b: &mut Builder, line: &Value, at: Option<u64>) {
        let msg = &line["message"];
        let content = &msg["content"];
        if line["isApiErrorMessage"] == true {
            let id = id(b, line, 0);
            let text = texts(content).trim().to_owned();
            return b.push(Item::Note { id, text });
        }
        let message = (msg["id"].as_str().or(line["requestId"].as_str())).unwrap_or_default();
        for (k, block) in content.as_array().into_iter().flatten().enumerate() {
            let id = id(b, line, k);
            match block["type"].as_str() {
                Some("text") => b.answer(message, id, at, block["text"].as_str().unwrap_or("")),
                // With thinking shown as updates, Claude Code keeps its
                // reasoning out and these are its notes to the user: the
                // terminal shows them as said, so the chat does too.
                Some("thinking") if line["thinkingDisplay"] == "updates" => {
                    b.answer(message, id, at, block["thinking"].as_str().unwrap_or(""))
                }
                Some("thinking") => b.thinking(id, block["thinking"].as_str().unwrap_or("")),
                Some("tool_use") => self.call(b, block, id, at),
                _ => {}
            }
        }
    }

    /// Tool call `block`: an edit with its change, or a tool with its gist.
    fn call(&mut self, b: &mut Builder, block: &Value, id: String, at: Option<u64>) {
        let id = block["id"].as_str().map_or(id, str::to_owned);
        let name = block["name"].as_str().unwrap_or("tool");
        let input = &block["input"];
        let path = (input["file_path"].as_str())
            .or(input["notebook_path"].as_str())
            .or(input["planFilePath"].as_str())
            .map(str::to_owned);
        let text = |key: &str| input[key].as_str().unwrap_or("");
        let plan = match (name, &path) {
            (super::live::PLAN_TOOL, _) => input["plan"].as_str(),
            ("Write", Some(path)) if is_plan_file(path) => Some(text("content")),
            _ => None,
        };
        if let Some(item) = plan.and_then(|plan| b.plan(id.clone(), path.clone(), plan)) {
            b.push_call(id, at, item);
            return;
        }
        let diff = match name {
            "Edit" => Some(change(text("old_string"), text("new_string"))),
            "MultiEdit" => Some(flat_edits(
                input["edits"].as_array().into_iter().flatten(),
                "old_string",
                "new_string",
            )),
            "Write" => Some(change("", text("content"))),
            "NotebookEdit" => Some(change("", text("new_source"))),
            _ => None,
        };
        let item = edit_or_tool(id.clone(), path.as_deref(), diff, || {
            if matches!(name, "Task" | "Agent") {
                let title = tool_gist(input).unwrap_or_else(|| name.to_owned());
                let prompt = text("prompt").to_owned();
                self.tasks.insert(id.clone(), (prompt, title));
            }
            let gist = tool_gist(input).unwrap_or_default();
            let input = (input["command"].as_str().map(str::to_owned))
                .unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default());
            (name.to_owned(), gist, input)
        });
        b.push_call(id, at, item);
    }
}

/// An id for the `k`th item of `line`: its uuid, else its number.
fn id(b: &Builder, line: &Value, k: usize) -> String {
    match line["uuid"].as_str() {
        Some(uuid) => format!("{uuid}.{k}"),
        None => format!("line{}.{k}", b.lines),
    }
}

/// What `text` of a line says: what the user typed (`from_user`), a slash
/// command as typed, a command's output, an interruption, a background
/// task's end; else a note of it.
fn told(text: &str, id: String, at: Option<u64>, from_user: bool) -> Option<Item> {
    let text = text.trim();
    let note = |text: String| {
        Some(Item::Note {
            id: id.clone(),
            text,
        })
    };
    if text.is_empty() || text.starts_with("<local-command-caveat>") {
        return None;
    }
    if text.starts_with("[Request interrupted by user") {
        return note(INTERRUPTED.to_owned());
    }
    if let Some(name) = tag(text, "command-name") {
        let slash = if name.starts_with('/') { "" } else { "/" };
        let args = tag(text, "command-args").unwrap_or("");
        let text = format!("{slash}{name} {args}").trim_end().to_owned();
        return Some(Item::User { id, at, text });
    }
    let out = tag(text, "local-command-stdout").or(tag(text, "local-command-stderr"));
    if let Some(out) = out {
        let out = strip_escapes(out).trim().to_owned();
        return (!out.is_empty()).then(|| note(out)).flatten();
    }
    if text.starts_with("<task-notification>") {
        return note(tag(text, "summary")?.trim().to_owned());
    }
    match from_user {
        true => Some(Item::User {
            id,
            at,
            text: text.to_owned(),
        }),
        false => note(text.to_owned()),
    }
}

/// The model of Claude Code transcript line `line` when it is a main
/// answer (not a subagent's) on a real model, by its id.
pub(super) fn main_model(line: &Value) -> Option<&str> {
    let model = line["message"]["model"].as_str()?;
    let main = line["type"] == "assistant" && line["isSidechain"] != true;
    (main && !model.is_empty() && model != "<synthetic>").then_some(model)
}

/// Claude model id `id` as people call it: `claude-opus-5-5` is "Opus 5.5",
/// a date at its end dropped. An id not of that shape stays as it is.
fn label(id: &str) -> String {
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
fn of_line(line: &Value) -> Option<String> {
    let msg = &line["message"];
    if line["type"] == "assistant" && line["isSidechain"] != true {
        return main_model(line).map(label);
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
pub(crate) fn last_in(tail: &[u8]) -> Option<String> {
    (tail.split(|b| *b == b'\n').rev())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find_map(|line| of_line(&line))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        chat::{DiffLine, DiffSide},
        integrations::common::build::MAX_LINES,
        md_blocks,
    };

    /// A reader of Claude fed `lines`, one JSON value a line.
    fn read(lines: &[Value]) -> Reader {
        let mut reader = Reader::new(AgentKind::Claude);
        reader.feed(&jsonl(lines));
        reader
    }

    fn jsonl(lines: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            out.extend(line.to_string().bytes());
            out.push(b'\n');
        }
        out
    }

    fn user(uuid: &str, text: &str) -> Value {
        serde_json::json!({
            "type": "user", "uuid": uuid, "timestamp": "2026-01-02T03:04:05.000Z",
            "message": { "role": "user", "content": text },
        })
    }

    fn said(uuid: &str, msg: &str, block: Value) -> Value {
        serde_json::json!({
            "type": "assistant", "uuid": uuid, "timestamp": "2026-01-02T03:04:06.000Z",
            "message": { "id": msg, "role": "assistant", "content": [block] },
        })
    }

    fn call(uuid: &str, id: &str, name: &str, input: Value) -> Value {
        let block =
            serde_json::json!({ "type": "tool_use", "id": id, "name": name, "input": input });
        said(uuid, &format!("msg-{uuid}"), block)
    }

    fn result(uuid: &str, id: &str, text: &str, error: bool) -> Value {
        serde_json::json!({
            "type": "user", "uuid": uuid, "timestamp": "2026-01-02T03:04:08.500Z",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": id, "content": text, "is_error": error },
            ]},
            "toolUseResult": { "stdout": text, "stderr": "", "interrupted": false },
        })
    }

    fn text(text: &str) -> Value {
        serde_json::json!({ "type": "text", "text": text })
    }

    #[test]
    fn a_result_pairs_with_its_call_across_feeds() {
        let mut reader = Reader::new(AgentKind::Claude);
        reader.feed(&jsonl(&[
            user("u1", "list the files"),
            call(
                "a1",
                "toolu_1",
                "Bash",
                serde_json::json!({ "command": "ls -la", "description": "List" }),
            ),
        ]));
        let Item::Tool {
            gist,
            input,
            output,
            ..
        } = &reader.items()[1]
        else {
            panic!("{:?}", reader.items());
        };
        assert_eq!(
            (gist.as_str(), input.as_str(), output),
            ("ls -la", "ls -la", &None)
        );
        reader.feed(&jsonl(&[result("r1", "toolu_1", "a\nb", false)]));
        assert_eq!(reader.items().len(), 2, "the result is no item of its own");
        let Item::Tool { id, output, .. } = &reader.items()[1] else {
            panic!()
        };
        assert_eq!(id, "toolu_1");
        let out = ToolOut {
            text: "a\nb".into(),
            failed: false,
            // From the timestamps: 06.000 to 08.500.
            ms: Some(2500),
        };
        assert_eq!(output.as_ref(), Some(&out));
    }

    #[test]
    fn a_streamed_message_is_one_answer() {
        let reader = read(&[
            user("u1", "hi"),
            said("a1", "msg_1", text("First part.")),
            said("a2", "msg_1", text("Second part.")),
            said("a3", "msg_2", text("Next message.")),
        ]);
        let items = reader.items();
        assert_eq!(items.len(), 3, "{items:?}");
        let Item::User { at, text, .. } = &items[0] else {
            panic!()
        };
        assert_eq!(
            (at, text.as_str()),
            (&iso_ms("2026-01-02T03:04:05.000Z"), "hi")
        );
        let Item::Assistant { id, blocks, .. } = &items[1] else {
            panic!()
        };
        assert_eq!(id, "a1.0");
        assert_eq!(**blocks, md_blocks::blocks("First part.\n\nSecond part."));
        assert_eq!(items[2].id(), "a3.0");
    }

    #[test]
    fn thinking_is_its_own_item() {
        let think = serde_json::json!({ "type": "thinking", "thinking": "Hmm.", "signature": "x" });
        let blank = serde_json::json!({ "type": "thinking", "thinking": "", "signature": "x" });
        let reader = read(&[
            said("a1", "msg_1", think),
            said("a2", "msg_1", blank),
            said("a3", "msg_1", text("Done.")),
        ]);
        let items = reader.items();
        let thinking = Item::Thinking {
            id: "a1.0".into(),
            text: "Hmm.".into(),
        };
        assert_eq!(items[0], thinking);
        assert!(matches!(items[1], Item::Assistant { .. }), "{items:?}");
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn thinking_shown_as_updates_is_said() {
        let note = serde_json::json!({ "type": "thinking", "thinking": "Reading the tail first.", "signature": "x" });
        let blank = serde_json::json!({ "type": "thinking", "thinking": "", "signature": "x" });
        let mut lines = [said("a1", "msg_1", note), said("a2", "msg_2", blank)];
        for line in &mut lines {
            line["thinkingDisplay"] = "updates".into();
        }
        let reader = read(&lines);
        let items = reader.items();
        assert_eq!(items.len(), 1, "{items:?}");
        let Item::Assistant { blocks, .. } = &items[0] else {
            panic!("{items:?}")
        };
        assert_eq!(**blocks, md_blocks::blocks("Reading the tail first."));
    }

    #[test]
    fn edits_show_their_change() {
        let edit =
            serde_json::json!({ "file_path": "/r/a.rs", "old_string": "a\nb", "new_string": "c" });
        let multi = serde_json::json!({ "file_path": "/r/b.rs", "edits": [
            { "old_string": "x", "new_string": "y" },
            { "old_string": "p", "new_string": "q" },
        ]});
        let reader = read(&[
            call("a1", "t1", "Edit", edit),
            call("a2", "t2", "MultiEdit", multi),
            result("r1", "t1", "The file was updated", false),
        ]);
        let line = |side, text: &str| DiffLine {
            side,
            text: text.into(),
        };
        use DiffSide::{Added, Removed};
        let Item::Edit {
            path, diff, output, ..
        } = &reader.items()[0]
        else {
            panic!()
        };
        assert_eq!(path, "/r/a.rs");
        assert_eq!(
            diff,
            &[line(Removed, "a"), line(Removed, "b"), line(Added, "c")]
        );
        assert!(output.as_ref().is_some_and(|o| !o.failed));
        let Item::Edit { diff, output, .. } = &reader.items()[1] else {
            panic!()
        };
        let want = [
            line(Removed, "x"),
            line(Added, "y"),
            line(Removed, "p"),
            line(Added, "q"),
        ];
        assert_eq!((diff.as_slice(), output), (want.as_slice(), &None));
    }

    #[test]
    fn a_write_is_its_lines_added_and_capped() {
        let content: String = (0..MAX_LINES + 5).map(|n| format!("line {n}\n")).collect();
        let reader = read(&[call(
            "a1",
            "t1",
            "Write",
            serde_json::json!({ "file_path": "/r/new.txt", "content": content }),
        )]);
        let Item::Edit { path, diff, .. } = &reader.items()[0] else {
            panic!()
        };
        assert_eq!(path, "/r/new.txt");
        assert_eq!(diff.len(), MAX_LINES + 1);
        assert!(diff[..MAX_LINES].iter().all(|l| l.side == DiffSide::Added));
        assert_eq!(diff[MAX_LINES].text, "\u{2026} 5 more lines");
    }

    #[test]
    fn a_plan_is_its_markdown_once() {
        let plan = "# Plan\n\n- one\n- two\n";
        let file = "/h/.claude/plans/x.md";
        let reader = read(&[
            call(
                "a1",
                "t1",
                "Write",
                serde_json::json!({ "file_path": file, "content": plan }),
            ),
            result("u1", "t1", "written", false),
            // Plan mode hands over what it just wrote: shown once.
            call(
                "a2",
                "t2",
                "ExitPlanMode",
                serde_json::json!({ "plan": plan }),
            ),
            call(
                "a3",
                "t3",
                "ExitPlanMode",
                serde_json::json!({ "plan": "# Other", "planFilePath": "/h/.claude/plans/y.md" }),
            ),
            call(
                "a4",
                "t4",
                "Edit",
                serde_json::json!({ "file_path": file, "old_string": "one", "new_string": "1" }),
            ),
        ]);
        let items = reader.items();
        let Item::Plan {
            path,
            blocks,
            output,
            ..
        } = &items[0]
        else {
            panic!("{:?}", items[0])
        };
        assert_eq!(path.as_deref(), Some(file));
        assert_eq!(**blocks, md_blocks::blocks(plan));
        assert_eq!(output.as_ref().map(|o| o.text.as_str()), Some("written"));
        assert!(matches!(&items[1], Item::Tool { name, .. } if name == "ExitPlanMode"));
        // Handed over, it names its file.
        assert!(matches!(&items[2], Item::Plan { path: Some(p), text, .. }
            if text == "# Other" && p == "/h/.claude/plans/y.md"));
        // A plan changed is a change.
        assert!(matches!(&items[3], Item::Edit { .. }));
    }

    #[test]
    fn a_failed_tool_says_so() {
        let reader = read(&[
            call(
                "a1",
                "t1",
                "Read",
                serde_json::json!({ "file_path": "/r/gone" }),
            ),
            result("r1", "t1", "File does not exist", true),
        ]);
        let Item::Tool {
            name, gist, output, ..
        } = &reader.items()[0]
        else {
            panic!()
        };
        assert_eq!((name.as_str(), gist.as_str()), ("Read", "/r/gone"));
        assert!(
            output
                .as_ref()
                .is_some_and(|o| o.failed && o.text == "File does not exist")
        );
    }

    #[test]
    fn a_subagents_work_goes_under_its_task() {
        let task = serde_json::json!({ "description": "Find the parser", "prompt": "Look for it", "subagent_type": "Explore" });
        let side = |mut line: Value, agent: &str| {
            line["isSidechain"] = true.into();
            line["agentId"] = agent.into();
            line
        };
        let mut done = result("r2", "task_1", "Found it", false);
        done["toolUseResult"] = serde_json::json!({ "agentId": "ag1", "totalDurationMs": 4200 });
        let reader = read(&[
            call("a1", "task_1", "Agent", task),
            side(user("s1", "Look for it"), "ag1"),
            side(
                call(
                    "s2",
                    "sub_t1",
                    "Grep",
                    serde_json::json!({ "pattern": "fn parse" }),
                ),
                "ag1",
            ),
            side(result("s3", "sub_t1", "src/a.rs", false), "ag1"),
            done,
            // Another agent id, no call of its own: left out.
            side(user("s4", "Someone else's"), "ag2"),
        ]);
        let items = reader.items();
        assert_eq!(items.len(), 1, "{items:?}");
        let Item::Subagent {
            id,
            title,
            items,
            output,
        } = &items[0]
        else {
            panic!()
        };
        assert_eq!(output.as_ref().and_then(|o| o.ms), Some(4200));
        assert_eq!(
            (id.as_str(), title.as_str()),
            ("agent:task_1", "Find the parser")
        );
        assert!(matches!(&items[0], Item::User { text, .. } if text == "Look for it"));
        assert!(matches!(&items[1], Item::Tool { output: Some(o), .. } if o.text == "src/a.rs"));
    }

    #[test]
    fn a_subagent_found_by_its_agent_id_from_the_result() {
        // An agent run in the background answers first, its work comes later.
        let task = serde_json::json!({ "description": "Late one", "prompt": "Go" });
        let mut done = result("r1", "task_1", "Started", false);
        done["toolUseResult"] =
            serde_json::json!({ "agentId": "ag1", "isAsync": true, "status": "async_launched" });
        let side = |mut line: Value, at: &str| {
            line["isSidechain"] = true.into();
            line["agentId"] = "ag1".into();
            line["timestamp"] = at.into();
            line
        };
        let reader = read(&[
            call("a1", "task_1", "Task", task),
            done,
            side(
                said("s1", "m9", text("Working.")),
                "2026-01-02T03:04:06.000Z",
            ),
            side(said("s2", "m10", text("Done.")), "2026-01-02T03:09:06.000Z"),
        ]);
        let Item::Subagent { items, output, .. } = &reader.items()[0] else {
            panic!()
        };
        assert!(matches!(&items[0], Item::Assistant { .. }));
        // Its own five minutes, not how long launching it took.
        assert_eq!(output.as_ref().and_then(|o| o.ms), Some(300_000));
    }

    #[test]
    fn a_subagents_own_file_goes_under_its_call() {
        let task = serde_json::json!({ "description": "Read the docs", "prompt": "Read them" });
        let mut reader = read(&[call("a1", "toolu_1", "Agent", task)]);
        assert!(reader.knows_call("toolu_1") && !reader.knows_call("toolu_9"));
        // Its file: lines of its own, no sidechain marks needed; begun
        // mid-line where only its end was read.
        let own = jsonl(&[user("s1", "Read them"), said("s2", "m1", text("Reading."))]);
        let cut = [&b"tail of a line\n"[..], &own].concat();
        reader.feed_subagent("ag1", "toolu_1", &cut);
        let Item::Subagent {
            title,
            items,
            output,
            ..
        } = &reader.items()[0]
        else {
            panic!("{:?}", reader.items())
        };
        assert_eq!(title, "Read the docs");
        assert_eq!(items.len(), 2);
        assert!(output.is_none(), "still running");
        // It grows; its call's result comes.
        reader.feed_subagent("ag1", "toolu_1", &jsonl(&[said("s3", "m2", text("Done."))]));
        reader.feed(&jsonl(&[result("r1", "toolu_1", "Report", false)]));
        let Item::Subagent { items, output, .. } = &reader.items()[0] else {
            panic!()
        };
        assert_eq!(items.len(), 3);
        assert_eq!(output.as_ref().map(|o| o.text.as_str()), Some("Report"));
        assert_eq!(reader.items().len(), 1);
    }

    #[test]
    fn notes_and_skips() {
        let lines = [
            serde_json::json!({ "type": "file-history-snapshot", "messageId": "m", "snapshot": {} }),
            serde_json::json!({ "type": "summary", "summary": "A title", "leafUuid": "x" }),
            serde_json::json!({ "type": "mystery", "uuid": "q1", "message": { "content": "?" } }),
            serde_json::json!({ "type": "user", "uuid": "m1", "isMeta": true,
                "message": { "content": "<local-command-caveat>Caveat</local-command-caveat>" } }),
            user(
                "c1",
                "<command-message>model</command-message>\n<command-name>/model</command-name>\n<command-args>opus</command-args>",
            ),
            user(
                "o1",
                "<local-command-stdout>Set model to \u{1b}[1mopus\u{1b}[22m</local-command-stdout>",
            ),
            serde_json::json!({ "type": "user", "uuid": "i1",
                "message": { "content": [text("[Request interrupted by user]")] } }),
            serde_json::json!({ "type": "system", "subtype": "compact_boundary", "uuid": "b1",
                "content": "Conversation compacted" }),
            serde_json::json!({ "type": "user", "uuid": "s1", "isCompactSummary": true,
                "message": { "content": "This session is being continued" } }),
            serde_json::json!({ "type": "system", "subtype": "turn_duration", "uuid": "d1", "durationMs": 5 }),
            serde_json::json!({ "type": "assistant", "uuid": "e1", "isApiErrorMessage": true,
                "message": { "id": "x", "model": "<synthetic>", "content": [text("API Error: overloaded")] } }),
            serde_json::json!({ "type": "attachment", "uuid": "q2", "attachment":
                { "type": "queued_command", "commandMode": "prompt", "prompt": "and also this" } }),
            serde_json::json!({ "type": "attachment", "uuid": "h1", "attachment":
                { "type": "hook_additional_context", "content": ["x"] } }),
        ];
        let got: Vec<Item> = read(&lines).items().to_vec();
        let note = |id: &str, text: &str| Item::Note {
            id: id.into(),
            text: text.into(),
        };
        let typed = |id: &str, text: &str| Item::User {
            id: id.into(),
            at: iso_ms("2026-01-02T03:04:05.000Z").filter(|_| id != "q2.0"),
            text: text.into(),
        };
        let want = vec![
            note("q1.0", "A \"mystery\" line gerdr can't show yet"),
            typed("c1.0", "/model opus"),
            note("o1.0", "Set model to opus"),
            note("i1.0", "Interrupted by the user"),
            note("b1.0", "Conversation compacted"),
            note("e1.0", "API Error: overloaded"),
            typed("q2.0", "and also this"),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn a_line_that_isnt_json_is_a_note_but_a_cut_first_one_is_left_out() {
        let mut reader = Reader::new(AgentKind::Claude);
        reader.feed(b"art\": \"cut short\"}\n");
        reader.feed(b"not json\n");
        let note = Item::Note {
            id: "line2".into(),
            text: "A line of the transcript gerdr couldn't read".into(),
        };
        assert_eq!(reader.items(), &[note]);
    }

    #[test]
    fn ids_hold_when_fed_in_two_parts() {
        let lines = jsonl(&[
            serde_json::json!({ "type": "user", "message": { "content": "no uuid" } }),
            call("a1", "t1", "Bash", serde_json::json!({ "command": "true" })),
            result("r1", "t1", "", false),
            said("a2", "m2", text("ok")),
        ]);
        let whole = read_bytes(&[&lines]);
        for cut in [1, 30, lines.len() / 2, lines.len() - 1] {
            let parts = read_bytes(&[&lines[..cut], &lines[cut..]]);
            assert_eq!(parts.items(), whole.items(), "cut at {cut}");
        }
        let ids: Vec<&str> = whole.items().iter().map(Item::id).collect();
        assert_eq!(ids, ["line1.0", "t1", "a2.0"]);
    }

    fn read_bytes(parts: &[&[u8]]) -> Reader {
        let mut reader = Reader::new(AgentKind::Claude);
        parts.iter().for_each(|p| reader.feed(p));
        reader
    }

    #[test]
    fn a_claude_line_means_nothing_to_codex() {
        let mut reader = Reader::new(AgentKind::Codex);
        reader.feed(&jsonl(&[user("u1", "hi")]));
        assert!(reader.items().is_empty());
    }

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
}
