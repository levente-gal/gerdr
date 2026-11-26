//! The Antigravity CLI's transcript of a conversation
//! (`~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/
//! transcript_full.jsonl`): a line per step, `{step_index, source, type,
//! status, created_at, ...}`. The user's input (`USER_INPUT`), the model's
//! turns (`PLANNER_RESPONSE`: thinking, its words, tool calls by name and
//! arguments only), each call's result as a step of its own after it
//! (`GENERIC`), in the order of the calls, and errors.
//!
//! A subagent (`invoke_subagent`, one call asks for one or more) is a
//! conversation of its own, with its own transcript; the call's result
//! names the conversations it created. Their steps go under the call's
//! card, read as they grow (`chat::tail`); a call that asked for several
//! holds a card per subagent.
//!
//! shortcut: a step's line is read once, as first written; read a step
//! again if Antigravity turns out to write one line per state of a step.
//!
//! shortcut: that the result names its conversations is read from the agy
//! binary (no subagent was at hand); else, a child conversation is the
//! call's by its first input holding a prompt the call gave. A subagent's
//! own subagents stay calls. Check both against a real transcript.

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use serde_json::Value;

use super::{Item, Reader, ToolOut};
use crate::{
    ai_work::{AgentKind, usage::iso_ms},
    integrations::{
        Parser,
        common::build::{Builder, Sub, cap, change, edit_or_tool, flat_edits, input_text},
    },
    stats_panel::tool_gist,
};

/// What a reader of the Antigravity CLI's keeps of its own.
#[derive(Default)]
pub(crate) struct Antigravity {
    /// Calls whose results come in their order, with no id to tell, first
    /// first.
    waiting: VecDeque<String>,
    /// The steps read, by their index.
    steps: HashSet<u64>,
    /// `invoke_subagent` calls and their subagents.
    spawns: Vec<Spawn>,
}

/// An `invoke_subagent` call and its subagents.
struct Spawn {
    call: String,
    /// Each subagent it asked for: its prompt and its card's title.
    asked: Vec<(String, String)>,
    /// The conversations its result named, once it came.
    named: Option<Vec<String>>,
    /// The conversations taken as its subagents: which one asked for, and
    /// its reader in `subs`; by the order asked.
    taken: Vec<(String, usize, usize)>,
}

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

impl Parser for Antigravity {
    /// Read transcript line `line`.
    fn line(&mut self, b: &mut Builder, line: &Value) {
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
                    b.push(Item::User { id, at, text });
                }
            }
            "PLANNER_RESPONSE" => {
                b.thinking(format!("{id}.t"), said("thinking"));
                b.answer(&id, format!("{id}.a"), at, said("content"));
                let calls = line["tool_calls"].as_array().into_iter().flatten();
                for (k, call) in calls.enumerate() {
                    self.call(b, call, format!("{id}.{k}"), at);
                }
            }
            // A call's result: the first call still waiting gets it.
            "GENERIC" if line["source"] == "MODEL" => {
                if let Some(call) = self.waiting.pop_front() {
                    b.fill(&call, outcome(said("content")));
                    if let Some(spawn) = self.spawns.iter_mut().find(|s| s.call == call) {
                        spawn.named = Some(conversation_ids(said("content")));
                    }
                }
            }
            "ERROR_MESSAGE" => {
                let text = said("error").lines().next().unwrap_or_default().to_owned();
                if !text.is_empty() {
                    b.push(Item::Note { id, text });
                }
            }
            // Antigravity's own messages to the model.
            _ => {}
        }
    }

    /// Whole lines of subagent `child`'s own transcript.
    fn subagent(&mut self, b: &mut Builder, child: &str, _call: &str, lines: &[u8]) {
        let Some(&sub) = b.sub_of.get(child) else {
            return;
        };
        b.subs[sub].reader.feed(lines);
        let spawn = (self.spawns.iter()).position(|s| s.taken.iter().any(|t| t.2 == sub));
        if let Some(s) = spawn {
            self.spawn_card(b, s);
        }
    }

    /// The conversations `invoke_subagent` results named that aren't taken
    /// as subagents yet, in order.
    fn named_subagents(&self, b: &Builder) -> Vec<String> {
        (self.spawns.iter())
            .flat_map(|s| s.named.iter().flatten())
            .filter(|id| !b.sub_of.contains_key(*id))
            .cloned()
            .collect()
    }

    /// How many subagents asked for have no conversation yet, of calls
    /// whose result came and named none.
    fn unfound_subagents(&self) -> usize {
        (self.spawns.iter())
            .filter(|s| s.named.as_ref().is_some_and(Vec::is_empty))
            .map(|s| s.asked.len() - s.taken.len())
            .sum()
    }

    /// The conversations taken as subagents.
    fn subagents(&self) -> Vec<String> {
        let taken = self.spawns.iter().flat_map(|s| &s.taken);
        taken.map(|(child, ..)| child.clone()).collect()
    }

    /// Take conversation `child` as a subagent: of the call whose result
    /// named it, else of a call that named none and gave the prompt its
    /// first input (`first`, its transcript's first line) holds. Whether
    /// it was taken.
    fn take_subagent(&mut self, b: &mut Builder, child: &str, first: &str) -> bool {
        if b.sub_of.contains_key(child) {
            return false;
        }
        let first = serde_json::from_str::<Value>(first).unwrap_or_default();
        let content = first["content"].as_str().unwrap_or_default();
        let input = match first["type"].as_str() {
            Some("USER_INPUT") => request(content),
            // agy 1.5 hands a subagent its prompt as its parent's message.
            Some("SYSTEM_MESSAGE") => content,
            _ => "",
        };
        let found = self.spawns.iter().enumerate().find_map(|(s, spawn)| {
            let named = spawn.named.as_ref()?;
            let open = |k: &usize| !spawn.taken.iter().any(|(_, at, _)| at == k);
            let mut ks = (0..spawn.asked.len()).filter(open);
            let k = match named.iter().any(|id| id == child) {
                true => ks.next(),
                false if named.is_empty() && !input.is_empty() => ks.find(|&k| {
                    let prompt = spawn.asked[k].0.trim();
                    !prompt.is_empty() && input.contains(prompt)
                }),
                false => None,
            };
            Some((s, k?))
        });
        let Some((s, k)) = found else {
            return false;
        };
        let Some(&(at, _)) = b.calls.get(&self.spawns[s].call) else {
            return false;
        };
        b.subs
            .push(Sub::new(at, Reader::new(AgentKind::Antigravity)));
        let sub = b.subs.len() - 1;
        b.sub_of.insert(child.to_owned(), sub);
        let taken = &mut self.spawns[s].taken;
        taken.push((child.to_owned(), k, sub));
        taken.sort_by_key(|(_, k, _)| *k);
        self.spawn_card(b, s);
        true
    }
}

impl Antigravity {
    /// Tool call `call` (`{name, args}`): a file written or replaced in with
    /// its change, else a tool with its gist.
    fn call(&mut self, b: &mut Builder, call: &Value, id: String, at: Option<u64>) {
        let name = call["name"].as_str().unwrap_or("tool");
        let args = &call["args"];
        let said = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_owned();
        let diff = match name {
            "write_to_file" => Some(change("", &said(args, "CodeContent"))),
            "replace_file_content" | "multi_replace_file_content" => Some(flat_edits(
                (args["ReplacementChunks"].as_array())
                    .map_or(std::slice::from_ref(args), Vec::as_slice),
                "TargetContent",
                "ReplacementContent",
            )),
            _ => None,
        };
        if name == "invoke_subagent" {
            // One subagent per call (`{TypeName, Role, Prompt}`, agy 1.5);
            // or a `Subagents` list of them.
            let asked = (args["Subagents"].as_array())
                .map_or(std::slice::from_ref(args), Vec::as_slice)
                .iter()
                .map(|entry| {
                    let prompt = said(entry, "Prompt");
                    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty());
                    let title = (entry["Role"].as_str().filter(|r| !r.is_empty()))
                        .or(line)
                        .or(entry["TypeName"].as_str())
                        .unwrap_or("Subagent");
                    let title = title.to_owned();
                    (prompt, title)
                })
                .collect();
            self.spawns.push(Spawn {
                call: id.clone(),
                asked,
                named: None,
                taken: Vec::new(),
            });
        }
        let item = edit_or_tool(id.clone(), args["TargetFile"].as_str(), diff, || {
            let gist = (GIST_KEYS.iter())
                .find_map(|k| args.get(*k).and_then(tool_gist))
                .unwrap_or_default();
            let input = match args["CommandLine"].as_str() {
                Some(command) => command.to_owned(),
                None => input_text(args),
            };
            (name.to_owned(), gist, input)
        });
        self.waiting.push_back(id.clone());
        b.push_call(id, at, item);
    }

    /// Spawn `s`'s card in its call's place: its subagent's steps, or a
    /// card per subagent when it asked for several.
    fn spawn_card(&self, b: &mut Builder, s: usize) {
        let spawn = &self.spawns[s];
        let Some(&(at, _)) = b.calls.get(&spawn.call) else {
            return;
        };
        let output = match &b.items[at] {
            Item::Tool { output, .. } | Item::Subagent { output, .. } => output.clone(),
            _ => None,
        };
        let own = |child: &str, sub: usize| Arc::new(prefixed(b.subs[sub].reader.items(), child));
        let (title, items) = match (&spawn.asked[..], &spawn.taken[..]) {
            ([(_, title)], [(child, _, sub)]) => (title.clone(), own(child, *sub)),
            (asked, taken) => {
                let cards = taken.iter().map(|(child, k, sub)| Item::Subagent {
                    id: format!("agent:{child}"),
                    title: asked[*k].1.clone(),
                    items: own(child, *sub),
                    output: output.clone(),
                });
                let title = format!("{} subagents", asked.len());
                (title, Arc::new(cards.collect()))
            }
        };
        b.items[at] = Item::Subagent {
            id: format!("agent:{}", spawn.call),
            title,
            items,
            output,
        };
    }
}

/// Subagent `child`'s `items`, their ids made its own: its steps are
/// numbered as the chat's are.
fn prefixed(items: &[Item], child: &str) -> Vec<Item> {
    let mut items = items.to_vec();
    for item in &mut items {
        let (Item::User { id, .. }
        | Item::Assistant { id, .. }
        | Item::Thinking { id, .. }
        | Item::Tool { id, .. }
        | Item::Edit { id, .. }
        | Item::Plan { id, .. }
        | Item::Subagent { id, .. }
        | Item::Note { id, .. }) = item;
        *id = format!("{child}:{id}");
    }
    items
}

/// The conversation ids (UUIDs) `text` names, each once, in order.
fn conversation_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let apart = |b: Option<&u8>| !b.is_some_and(u8::is_ascii_alphanumeric);
    let mut ids: Vec<String> = Vec::new();
    for at in 0..bytes.len().saturating_sub(35) {
        let word = &bytes[at..at + 36];
        let shaped = word.iter().enumerate().all(|(k, b)| match k {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
        let before = at.checked_sub(1).and_then(|i| bytes.get(i));
        if shaped && apart(before) && apart(bytes.get(at + 36)) {
            let id = String::from_utf8_lossy(word).into_owned();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
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
        reader.items().to_vec()
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

    const KID: &str = "0b9c4f2e-1d3a-4c5b-8e7f-a1b2c3d4e5f6";
    const KID2: &str = "1c9c4f2e-1d3a-4c5b-8e7f-a1b2c3d4e5f6";

    fn spawn(prompts: &[&str]) -> Value {
        let asked: Vec<Value> = (prompts.iter())
            .map(|p| json!({"TypeName": "self", "Prompt": p}))
            .collect();
        step(
            1,
            "PLANNER_RESPONSE",
            "MODEL",
            json!({"tool_calls": [{"name": "invoke_subagent", "args": {"Subagents": asked}}]}),
        )
    }

    fn lines(lines: &[Value]) -> Vec<u8> {
        (lines.iter())
            .flat_map(|line| format!("{line}\n").into_bytes())
            .collect()
    }

    fn input(n: u64, text: &str) -> Value {
        let content = format!("<USER_REQUEST>\n{text}\n</USER_REQUEST>");
        step(
            n,
            "USER_INPUT",
            "USER_EXPLICIT",
            json!({"content": content}),
        )
    }

    #[test]
    fn a_subagent_its_result_names_shows_under_its_call() {
        let mut reader = Reader::new(AgentKind::Antigravity);
        reader.feed(&lines(&[
            spawn(&["Find the parser\nthen say where"]),
            result(
                2,
                &format!("Created the following subagent:\n- self ({KID})\n"),
            ),
        ]));
        assert_eq!(reader.named_subagents(), [KID]);
        assert_eq!(reader.unfound_subagents(), 0);
        assert!(reader.take_subagent(KID, ""));
        assert!(!reader.take_subagent(KID, ""), "taken once");
        assert!(reader.named_subagents().is_empty());
        reader.feed_subagent(KID, "", &lines(&[input(0, "Find the parser")]));
        let child = step(
            1,
            "PLANNER_RESPONSE",
            "MODEL",
            json!({"content": "In src."}),
        );
        reader.feed_subagent(KID, "", &lines(&[child]));
        let [
            Item::Subagent {
                id,
                title,
                items,
                output: Some(_),
            },
        ] = reader.items()
        else {
            panic!("{:?}", reader.items())
        };
        assert_eq!(
            (id.as_str(), title.as_str()),
            ("agent:step1.0", "Find the parser")
        );
        let ids: Vec<&str> = items.iter().map(Item::id).collect();
        assert_eq!(ids, [format!("{KID}:step0"), format!("{KID}:step1.a")]);
    }

    #[test]
    fn subagents_a_result_names_not_are_found_by_their_prompt() {
        let mut reader = Reader::new(AgentKind::Antigravity);
        reader.feed(&lines(&[spawn(&["Read a", "Read b"])]));
        // No result yet: none to find.
        assert_eq!(reader.unfound_subagents(), 0);
        reader.feed(&lines(&[result(2, "Created the following 2 subagents:\n")]));
        assert_eq!(reader.unfound_subagents(), 2);
        let first = |text: &str| input(0, text).to_string();
        assert!(!reader.take_subagent("other", &first("Write c")));
        // agy 1.5 gives its prompt as the parent's message.
        let message = step(
            0,
            "SYSTEM_MESSAGE",
            "SYSTEM",
            json!({"content": "<SYSTEM_MESSAGE>\n[Message] sender=x content=Read b\n</SYSTEM_MESSAGE>"}),
        );
        assert!(reader.take_subagent(KID2, &message.to_string()));
        assert!(reader.take_subagent(KID, &first("Read a")));
        assert_eq!(reader.unfound_subagents(), 0);
        let [Item::Subagent { title, items, .. }] = reader.items() else {
            panic!("{:?}", reader.items())
        };
        assert_eq!(title, "2 subagents");
        // A card each, in the order asked.
        let cards: Vec<&str> = (items.iter())
            .map(|item| match item {
                Item::Subagent { title, .. } => title.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(cards, ["Read a", "Read b"]);
        assert_eq!(reader.subagents(), [KID, KID2]);
    }

    /// agy 1.5's shape: one subagent per call, each call its own result.
    #[test]
    fn calls_of_one_subagent_each_get_their_own_card() {
        let call = |role: &str, file: &str| {
            json!({"name": "invoke_subagent",
                   "args": {"Prompt": format!("Read {file}"), "Role": role, "TypeName": "research"}})
        };
        let made = |kid: &str| {
            format!("Created the following subagent:\n{{\n  \"conversationId\":  \"{kid}\"\n}}\n")
        };
        let mut reader = Reader::new(AgentKind::Antigravity);
        reader.feed(&lines(&[
            step(
                1,
                "PLANNER_RESPONSE",
                "MODEL",
                json!({"tool_calls": [call("File A Reader", "a.txt"), call("File B Reader", "b.txt")]}),
            ),
            result(2, &made(KID)),
            result(3, &made(KID2)),
        ]));
        assert_eq!(reader.named_subagents(), [KID, KID2]);
        for kid in [KID2, KID] {
            assert!(reader.take_subagent(kid, ""));
            reader.feed_subagent(kid, "", &lines(&[input(0, "Read it")]));
        }
        let titles: Vec<(&str, &str)> = (reader.items().iter())
            .map(|item| match item {
                Item::Subagent { id, title, .. } => (id.as_str(), title.as_str()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            titles,
            [
                ("agent:step1.0", "File A Reader"),
                ("agent:step1.1", "File B Reader")
            ]
        );
    }

    #[test]
    fn conversation_ids_are_whole_uuids() {
        let text = format!("{KID} x{KID2} ({KID2}) {KID}0 {KID}");
        assert_eq!(conversation_ids(&text), [KID, KID2]);
        assert!(conversation_ids("short").is_empty());
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
