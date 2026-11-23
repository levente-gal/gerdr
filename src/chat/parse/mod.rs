//! A transcript read into chat items, as it grows.
//!
//! Claude Code's (here): a JSON line per user turn, per block of an
//! assistant message (its lines share `message.id`), per tool result, plus
//! lines of its own bookkeeping. Subagents' lines (`isSidechain`) go in a
//! `Subagent` under the `Task`/`Agent` call that started them.
//!
//! Codex's rollout is lines too, fed the same way (`codex`). OpenCode's and
//! Cursor's chats are in databases: their rows are read whole each time they
//! change, and `snapshot` makes the items of all of them (`opencode`,
//! `cursor`). Item ids come from the transcript (a message's, a call's), so
//! they stay the same when it is read again and folds keep.

mod codex;
mod cursor;
mod opencode;
pub(crate) mod patch;

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde_json::Value;

use super::{DiffLine, DiffSide, Item, ToolOut};
use crate::{
    ai_work::{AgentKind, usage::iso_ms},
    feature::verify::strip_escapes,
    md_blocks,
    stats_panel::tool_gist,
};

// shortcut: a tool's output keeps its first 2000 lines or 200 KB and an
// edit's diff its first 2000 lines; read the rest from the file on request
// if cut outputs turn out to matter.
const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 200 * 1024;

/// Reads one chat's transcript, fed its bytes as they come.
pub struct Reader {
    agent: AgentKind,
    items: Vec<Item>,
    /// The start of a line whose end hasn't come yet.
    rest: Vec<u8>,
    /// Lines read so far: ids for lines without their own.
    lines: usize,
    /// Tool calls by their id: their item, and when they were made.
    calls: HashMap<String, (usize, Option<u64>)>,
    /// The last item's markdown while it is an answer, with its message id:
    /// the rest of that message joins it.
    answer: Option<(String, String)>,
    /// `Task`/`Agent` calls whose work hasn't shown yet, by their id: the
    /// prompt they gave and their title.
    tasks: HashMap<String, (String, String)>,
    /// A subagent's call by its agent id, as its result says.
    agent_calls: HashMap<String, String>,
    subs: Vec<Sub>,
    /// Which of `subs` a sidechain line is of, by its agent id or by a line's
    /// uuid (older transcripts have no agent ids).
    sub_of: HashMap<String, usize>,
}

/// A subagent's work: its item, read by a reader of its own.
struct Sub {
    at: usize,
    reader: Reader,
    changed: bool,
}

impl Reader {
    pub fn new(agent: AgentKind) -> Self {
        Self {
            agent,
            items: Vec::new(),
            rest: Vec::new(),
            lines: 0,
            calls: HashMap::new(),
            answer: None,
            tasks: HashMap::new(),
            agent_calls: HashMap::new(),
            subs: Vec::new(),
            sub_of: HashMap::new(),
        }
    }

    /// Read `lines`, whole lines of the transcript after those fed before
    /// (each ending in `\n`). An item already read can change: a tool call
    /// gets its result.
    pub fn feed(&mut self, lines: &[u8]) {
        // OpenCode's and Cursor's are read with `snapshot`.
        if !matches!(self.agent, AgentKind::Claude | AgentKind::Codex) {
            return;
        }
        self.rest.extend_from_slice(lines);
        let Some(end) = self.rest.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let whole: Vec<u8> = self.rest.drain(..=end).collect();
        for raw in whole[..end].split(|b| *b == b'\n') {
            self.lines += 1;
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<Value>(raw) {
                Ok(line) if self.agent == AgentKind::Codex => self.codex_line(&line),
                Ok(line) => self.route(&line),
                // Reading can start mid-file: the first line may be cut short.
                Err(_) if self.lines == 1 => {}
                Err(_) => {
                    let id = format!("line{}", self.lines);
                    let text = "A line of the transcript gerdr couldn't read".to_owned();
                    self.push(Item::Note { id, text });
                }
            }
        }
        for sub in self.subs.iter_mut().filter(|s| s.changed) {
            sub.changed = false;
            if let Item::Subagent { items, .. } = &mut self.items[sub.at] {
                *items = Arc::new(sub.reader.items.clone());
            }
        }
    }

    /// The items read so far, in order.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Item `item` set at `call`, the tool call's id, so its result
    /// (`fill`) finds it.
    fn push_call(&mut self, call: String, at: Option<u64>, item: Item) {
        self.calls.insert(call, (self.items.len(), at));
        self.push(item);
    }

    /// The result of tool call `call`, given to its item; false when the
    /// call isn't known (made before reading began).
    fn fill(&mut self, call: &str, out: ToolOut) -> bool {
        let Some(&(index, _)) = self.calls.get(call) else {
            return false;
        };
        if let Item::Tool { output, .. } | Item::Edit { output, .. } = &mut self.items[index] {
            *output = Some(out);
        }
        true
    }

    /// `line` read here, or in its subagent's reader.
    fn route(&mut self, line: &Value) {
        if line["isSidechain"] != true {
            return self.line(line);
        }
        let agent = line["agentId"].as_str();
        let known = (agent.and_then(|a| self.sub_of.get(a)))
            .or_else(|| line["parentUuid"].as_str().and_then(|p| self.sub_of.get(p)))
            .copied();
        // A subagent whose call can't be told is left out.
        let Some(sub) = known.or_else(|| self.new_sub(line, agent)) else {
            return;
        };
        if let (None, Some(uuid)) = (agent, line["uuid"].as_str()) {
            self.sub_of.insert(uuid.to_owned(), sub);
        }
        let sub = &mut self.subs[sub];
        sub.reader.lines = self.lines;
        sub.reader.line(line);
        sub.changed = true;
    }

    /// A subagent for sidechain `line`, when the call that started it is
    /// known: by its agent id, or by its first line being the call's prompt.
    fn new_sub(&mut self, line: &Value, agent: Option<&str>) -> Option<usize> {
        let by_agent = agent.and_then(|a| self.agent_calls.get(a)).cloned();
        let call = by_agent.or_else(|| {
            let prompt = (line["type"] == "user").then(|| texts(&line["message"]["content"]))?;
            let prompt = prompt.trim();
            let found = self.tasks.iter().find(|(_, (p, _))| p.trim() == prompt);
            found.map(|(id, _)| id.clone())
        })?;
        let (_, title) = self.tasks.remove(&call)?;
        self.subs.push(Sub {
            at: self.items.len(),
            reader: Reader::new(self.agent),
            changed: false,
        });
        let id = format!("agent:{call}");
        self.push(Item::Subagent {
            id,
            title,
            items: Arc::default(),
        });
        let sub = self.subs.len() - 1;
        if let Some(agent) = agent {
            self.sub_of.insert(agent.to_owned(), sub);
        }
        Some(sub)
    }

    fn line(&mut self, line: &Value) {
        let at = line["timestamp"].as_str().and_then(iso_ms);
        match line["type"].as_str().unwrap_or_default() {
            "user" => self.user(line, at),
            "assistant" => self.assistant(line, at),
            "system" if line["isMeta"] != true => {
                if let Some(text) = line["content"].as_str() {
                    let id = self.id(line, 0);
                    self.push_some(told(text, id, at, false));
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
                    let id = self.id(line, 0);
                    self.push_some(told(text, id, at, true));
                }
            }
            // The API request again, as sent.
            "api-request-blob" => {}
            // What has no message is Claude Code's bookkeeping: titles,
            // modes, file snapshots, costs.
            kind if line.get("message").is_some() => {
                let id = self.id(line, 0);
                let text = format!("A \"{kind}\" line gerdr can't show yet");
                self.push(Item::Note { id, text });
            }
            _ => {}
        }
    }

    fn user(&mut self, line: &Value, at: Option<u64>) {
        let content = &line["message"]["content"];
        for part in content.as_array().into_iter().flatten() {
            if part["type"] == "tool_result" {
                self.result(part, line, at);
            }
        }
        // Meta lines are what Claude Code tells the model; compaction
        // summaries too (the "compacted" system line notes it).
        if line["isMeta"] == true || line["isCompactSummary"] == true {
            return;
        }
        let id = self.id(line, 0);
        self.push_some(told(&texts(content), id, at, true));
    }

    /// Tool result `part` of user `line`, given to its call's item.
    fn result(&mut self, part: &Value, line: &Value, at: Option<u64>) {
        let Some(call) = part["tool_use_id"].as_str() else {
            return;
        };
        // A call before where reading started has nothing to fill.
        let Some(&(index, called)) = self.calls.get(call) else {
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
            .or_else(|| at?.checked_sub(called?));
        let out = ToolOut {
            text: cap(&texts(&part["content"])),
            failed: part["is_error"] == true,
            ms,
        };
        if let Item::Tool { output, .. } | Item::Edit { output, .. } = &mut self.items[index] {
            *output = Some(out);
        }
        if let Some(agent) = done["agentId"].as_str() {
            self.agent_calls.insert(agent.to_owned(), call.to_owned());
        }
    }

    fn assistant(&mut self, line: &Value, at: Option<u64>) {
        let msg = &line["message"];
        let content = &msg["content"];
        if line["isApiErrorMessage"] == true {
            let id = self.id(line, 0);
            let text = texts(content).trim().to_owned();
            return self.push(Item::Note { id, text });
        }
        let message = (msg["id"].as_str().or(line["requestId"].as_str())).unwrap_or_default();
        for (k, block) in content.as_array().into_iter().flatten().enumerate() {
            let id = self.id(line, k);
            match block["type"].as_str() {
                Some("text") => self.answer(message, id, at, block["text"].as_str().unwrap_or("")),
                Some("thinking") => {
                    let text = block["thinking"].as_str().unwrap_or("").trim();
                    if !text.is_empty() {
                        let text = text.to_owned();
                        self.push(Item::Thinking { id, text });
                    }
                }
                Some("tool_use") => self.call(block, id, at),
                _ => {}
            }
        }
    }

    /// Text of an answer: joined to the last item when that is the same
    /// message's text.
    fn answer(&mut self, message: &str, id: String, at: Option<u64>, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some((open, md)) = &mut self.answer
            && !message.is_empty()
            && open == message
            && let Some(Item::Assistant { blocks, .. }) = self.items.last_mut()
        {
            md.push_str("\n\n");
            md.push_str(text);
            *blocks = Arc::new(md_blocks::blocks(md));
            return;
        }
        let blocks = Arc::new(md_blocks::blocks(text));
        self.push(Item::Assistant { id, at, blocks });
        self.answer = Some((message.to_owned(), text.to_owned()));
    }

    /// Tool call `block`: an edit with its change, or a tool with its gist.
    fn call(&mut self, block: &Value, id: String, at: Option<u64>) {
        let id = block["id"].as_str().map_or(id, str::to_owned);
        let name = block["name"].as_str().unwrap_or("tool");
        let input = &block["input"];
        let path = (input["file_path"].as_str())
            .or(input["notebook_path"].as_str())
            .map(str::to_owned);
        let text = |key: &str| input[key].as_str().unwrap_or("");
        let diff = match name {
            "Edit" => Some(change(text("old_string"), text("new_string"))),
            "MultiEdit" => Some(
                (input["edits"].as_array().into_iter().flatten())
                    .flat_map(|e| {
                        let said = |key: &str| e[key].as_str().unwrap_or("");
                        change(said("old_string"), said("new_string"))
                    })
                    .collect(),
            ),
            "Write" => Some(change("", text("content"))),
            "NotebookEdit" => Some(change("", text("new_source"))),
            _ => None,
        };
        let item = match (path, diff) {
            (Some(path), Some(mut diff)) => {
                if diff.len() > MAX_LINES {
                    let text = more(diff.len() - MAX_LINES);
                    diff.truncate(MAX_LINES);
                    let side = DiffSide::Context;
                    diff.push(DiffLine { side, text });
                }
                Item::Edit {
                    id: id.clone(),
                    path,
                    diff,
                    output: None,
                }
            }
            _ => {
                if matches!(name, "Task" | "Agent") {
                    let title = tool_gist(input).unwrap_or_else(|| name.to_owned());
                    let prompt = text("prompt").to_owned();
                    self.tasks.insert(id.clone(), (prompt, title));
                }
                Item::Tool {
                    id: id.clone(),
                    name: name.to_owned(),
                    gist: tool_gist(input).unwrap_or_default(),
                    input: (input["command"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default()),
                    output: None,
                }
            }
        };
        self.calls.insert(id, (self.items.len(), at));
        self.push(item);
    }

    /// An id for the `k`th item of `line`: its uuid, else its number.
    fn id(&self, line: &Value, k: usize) -> String {
        match line["uuid"].as_str() {
            Some(uuid) => format!("{uuid}.{k}"),
            None => format!("line{}.{k}", self.lines),
        }
    }

    fn push(&mut self, item: Item) {
        self.answer = None;
        self.items.push(item);
    }

    fn push_some(&mut self, item: Option<Item>) {
        if let Some(item) = item {
            self.push(item);
        }
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
        return note("Interrupted by the user".to_owned());
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

/// The text between `<name>` and `</name>` in `text`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let len = text[start..].find(&format!("</{name}>"))?;
    Some(&text[start..start + len])
}

/// The words of message content: a string, or its text parts; an image as
/// "[image]".
fn texts(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => (parts.iter())
            .filter_map(|part| match part["type"].as_str() {
                Some("text") => part["text"].as_str().map(str::to_owned),
                Some("image") => Some("[image]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// `old` replaced by `new`, as lines taken out and put in.
fn change(old: &str, new: &str) -> Vec<DiffLine> {
    let lines = |text: &str, side| {
        (text.lines())
            .map(move |l| DiffLine {
                side,
                text: l.to_owned(),
            })
            .collect::<Vec<_>>()
    };
    let mut diff = lines(old, DiffSide::Removed);
    diff.extend(lines(new, DiffSide::Added));
    diff
}

/// `text` kept to its first `MAX_LINES` lines and `MAX_BYTES` bytes, saying
/// how many lines it left out.
fn cap(text: &str) -> String {
    let mut end = 0;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        if n == MAX_LINES || end + line.len() > MAX_BYTES {
            // One line too long by itself is cut within.
            if end == 0 {
                end = text.floor_char_boundary(MAX_BYTES);
            }
            let left = text[end..].lines().count();
            return format!("{}\n{}", text[..end].trim_end_matches('\n'), more(left));
        }
        end += line.len();
    }
    text.to_owned()
}

fn more(lines: usize) -> String {
    match lines {
        1 => "\u{2026} 1 more line".to_owned(),
        n => format!("\u{2026} {n} more lines"),
    }
}

/// The items of a chat kept in a database, from all its rows in order:
/// OpenCode's message rows as `ai_work::locate` reads them (a `message`
/// row with its `parts`, or a `session_message` row, its JSON columns as
/// what they hold), Cursor's `store.db` messages as
/// `ai_work::usage::cursor_messages` gives them. Other agents' have none.
pub fn snapshot(agent: AgentKind, rows: &[Value]) -> Vec<Item> {
    let mut reader = Reader::new(agent);
    match agent {
        AgentKind::OpenCode => rows.iter().for_each(|row| reader.opencode_row(row)),
        AgentKind::Cursor => {
            (rows.iter().enumerate()).for_each(|(n, row)| reader.cursor_row(n, row))
        }
        _ => {}
    }
    reader.items
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn long_output_is_capped() {
        let long: String = (0..MAX_LINES + 2).map(|n| format!("{n}\n")).collect();
        let kept = cap(&long);
        assert_eq!(kept.lines().count(), MAX_LINES + 1);
        assert!(
            kept.ends_with("\u{2026} 2 more lines"),
            "{}",
            &kept[kept.len() - 40..]
        );
        let wide = "é".repeat(MAX_BYTES);
        assert!(cap(&wide).len() <= MAX_BYTES + 40);
        assert_eq!(cap("short\n"), "short\n");
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
        assert_eq!(items.len(), 2, "{items:?}");
        let Item::Tool { output, .. } = &items[0] else {
            panic!()
        };
        assert_eq!(output.as_ref().and_then(|o| o.ms), Some(4200));
        let Item::Subagent { id, title, items } = &items[1] else {
            panic!()
        };
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
        done["toolUseResult"] = serde_json::json!({ "agentId": "ag1", "isAsync": true });
        let mut work = said("s1", "m9", text("Working."));
        work["isSidechain"] = true.into();
        work["agentId"] = "ag1".into();
        let reader = read(&[call("a1", "task_1", "Task", task), done, work]);
        let Item::Subagent { items, .. } = &reader.items()[1] else {
            panic!()
        };
        assert!(matches!(&items[0], Item::Assistant { .. }));
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
    fn other_agents_read_nothing_yet() {
        let mut reader = Reader::new(AgentKind::Codex);
        reader.feed(&jsonl(&[user("u1", "hi")]));
        assert!(reader.items().is_empty());
    }
}
