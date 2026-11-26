//! Claude Code's transcript: a JSON line per user turn, per block of an
//! assistant message (its lines share `message.id`), per tool result, plus
//! lines of its own bookkeeping. Subagents' lines (`isSidechain`) go in a
//! `Subagent` under the `Task`/`Agent` call that started them.

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde_json::Value;

use super::{Item, Reader, ToolOut};
use crate::{
    ai_work::{AgentKind, usage::iso_ms},
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
            if let Some(model) = crate::chat::model::of_line(line) {
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
            (crate::chat::live::PLAN_TOOL, _) => input["plan"].as_str(),
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
