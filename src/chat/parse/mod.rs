//! A transcript read into chat items, as it grows.
//!
//! Claude Code's (`claude`), Codex's rollout (`codex`), pi's and the
//! Antigravity CLI's are lines, fed as they come. OpenCode's and
//! Cursor's chats are in databases: their rows are read whole each time they
//! change, and `snapshot` makes the items of all of them (`opencode`,
//! `cursor`). Item ids come from the transcript (a message's, a call's), so
//! they stay the same when it is read again and folds keep.

pub(crate) mod claude;
pub(crate) mod patch;
pub(crate) mod pi;

pub(crate) use crate::integrations::opencode::parse::gist;

use serde_json::Value;

use super::{Item, ToolOut};
#[cfg(test)]
pub(crate) use crate::integrations::common::build::plan_item;
use crate::{
    ai_work::AgentKind,
    integrations::{Parser, Store, common::build::Builder},
};

/// Reads one chat's transcript, fed its bytes as they come: what all
/// agents' readers keep, and what its agent's keeps of its own.
pub struct Reader {
    pub(crate) b: Builder,
    pub(crate) p: Box<dyn Parser>,
    /// Whether its chat is lines, fed; a database chat's rows are read
    /// with `snapshot`.
    lines: bool,
}

impl Reader {
    pub fn new(agent: AgentKind) -> Self {
        let integration = crate::integrations::of(agent);
        Self {
            b: Builder::default(),
            p: integration.parser(),
            lines: integration.store() == Some(Store::Lines),
        }
    }

    /// Read `lines`, whole lines of the transcript after those fed before
    /// (each ending in `\n`). An item already read can change: a tool call
    /// gets its result.
    pub fn feed(&mut self, lines: &[u8]) {
        if !self.lines {
            return;
        }
        let b = &mut self.b;
        b.rest.extend_from_slice(lines);
        let Some(end) = b.rest.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let whole: Vec<u8> = b.rest.drain(..=end).collect();
        for raw in whole[..end].split(|b| *b == b'\n') {
            b.lines += 1;
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<Value>(raw) {
                Ok(line) => self.p.line(b, &line),
                // Reading can start mid-file: the first line may be cut short.
                Err(_) if b.lines == 1 => {}
                Err(_) => {
                    let id = format!("line{}", b.lines);
                    let text = "A line of the transcript gerdr couldn't read".to_owned();
                    b.push(Item::Note { id, text });
                }
            }
        }
        b.sync_subs();
    }

    /// Whole lines of subagent `agent`'s own transcript, apart from the
    /// chat's; `call` is the call that started it, as the agent says.
    pub fn feed_subagent(&mut self, agent: &str, call: &str, lines: &[u8]) {
        self.p.subagent(&mut self.b, agent, call, lines);
        self.b.sync_subs();
    }

    /// Whether tool call `call` was read: a subagent it started has a
    /// place to show.
    pub fn knows_call(&self, call: &str) -> bool {
        self.b.knows_call(call)
    }

    /// The model the chat is on, as its lines read so far last said.
    pub fn model(&self) -> Option<String> {
        self.p.model()
    }

    /// The items read so far, in order.
    pub fn items(&self) -> &[Item] {
        self.b.items()
    }

    /// The conversations results named as subagents not taken yet.
    pub fn named_subagents(&self) -> Vec<String> {
        self.p.named_subagents(&self.b)
    }

    /// How many subagents asked for have no conversation yet.
    pub fn unfound_subagents(&self) -> usize {
        self.p.unfound_subagents()
    }

    /// The conversations taken as subagents.
    pub fn subagents(&self) -> Vec<String> {
        self.p.subagents()
    }

    /// Take conversation `child` as a subagent, its transcript's first
    /// line `first`; whether it was taken.
    pub fn take_subagent(&mut self, child: &str, first: &str) -> bool {
        self.p.take_subagent(&mut self.b, child, first)
    }
}

/// The items of a chat kept in a database, from all its rows in order:
/// OpenCode's message rows as `ai_work::locate` reads them (a `message`
/// row with its `parts`, or a `session_message` row, its JSON columns as
/// what they hold), Cursor's `store.db` messages as
/// `ai_work::usage::cursor_messages` gives them. Other agents' have none.
pub fn snapshot<'a>(agent: AgentKind, rows: impl IntoIterator<Item = &'a Value>) -> Vec<Item> {
    let mut reader = Reader::new(agent);
    reader.p.rows(&mut reader.b, &mut rows.into_iter());
    reader.b.items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ai_work::usage::iso_ms,
        chat::{DiffLine, DiffSide},
        integrations::common::build::{MAX_BYTES, MAX_LINES, cap},
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
}
