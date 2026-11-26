//! What every agent's chat reader shares: the items read so far, the
//! calls they hold, and the helpers that make items of what agents say.
//! What only one agent keeps is in its `Parser`.

use std::{collections::HashMap, sync::Arc};

use serde_json::Value;

use crate::{
    chat::{DiffLine, DiffSide, Item, ToolOut, parse::Reader},
    md_blocks,
    stats_panel::tool_gist,
};

// shortcut: a tool's output keeps its first 2000 lines or 200 KB and an
// edit's diff its first 2000 lines; read the rest from the file on request
// if cut outputs turn out to matter.
pub const MAX_LINES: usize = 2000;
pub const MAX_BYTES: usize = 200 * 1024;

/// Said where the user stopped the agent.
pub const INTERRUPTED: &str = "Interrupted by the user";
/// Said where Cursor's and OpenCode's chats were compacted.
pub const COMPACTED: &str = "Conversation compacted";
/// Said where Codex's and pi's chats were compacted.
pub const CHAT_COMPACTED: &str = "The chat was compacted";

/// Said of a line of kind `kind` that isn't shown.
pub fn cant_show(kind: &str) -> String {
    format!("A \"{kind}\" line gerdr can't show yet")
}

/// A chat's items as they are read.
#[derive(Default)]
pub struct Builder {
    pub items: Vec<Item>,
    /// The start of a line whose end hasn't come yet.
    pub rest: Vec<u8>,
    /// Lines read so far: ids for lines without their own.
    pub lines: usize,
    /// Tool calls by their id: their item, and when they were made.
    pub calls: HashMap<String, (usize, Option<u64>)>,
    /// The last item's markdown while it is an answer, with its message id:
    /// the rest of that message joins it.
    pub answer: Option<(String, String)>,
    /// The last plan shown, trimmed: plan mode hands over the plan it just
    /// wrote to its file, and it shows once.
    pub plan: Option<String>,
    pub subs: Vec<Sub>,
    /// Which of `subs` a subagent's line is of: by its agent id, a line's
    /// uuid or its conversation.
    pub sub_of: HashMap<String, usize>,
}

/// A subagent's work: its item, read by a reader of its own.
pub struct Sub {
    pub at: usize,
    pub reader: Reader,
    pub changed: bool,
    /// When its first and last lines read were written.
    pub span: Option<(u64, u64)>,
    /// Its card's time is that span, kept up as it grows.
    pub spanned: bool,
}

impl Sub {
    /// The subagent of the item at `at`, read by `reader`.
    pub fn new(at: usize, reader: Reader) -> Self {
        Self {
            at,
            reader,
            changed: false,
            span: None,
            spanned: false,
        }
    }
}

impl Builder {
    pub fn push(&mut self, item: Item) {
        self.answer = None;
        self.items.push(item);
    }

    pub fn push_some(&mut self, item: Option<Item>) {
        if let Some(item) = item {
            self.push(item);
        }
    }

    /// Item `item` set at `call`, the tool call's id, so its result
    /// (`fill`) finds it.
    pub fn push_call(&mut self, call: String, at: Option<u64>, item: Item) {
        self.calls.insert(call, (self.items.len(), at));
        self.push(item);
    }

    /// Thinking `text`, trimmed, as its item; none when it says nothing.
    pub fn thinking(&mut self, id: String, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            let text = text.to_owned();
            self.push(Item::Thinking { id, text });
        }
    }

    /// The result of tool call `call`, given to its item; false when the
    /// call isn't known (made before reading began).
    pub fn fill(&mut self, call: &str, out: ToolOut) -> bool {
        let Some(&(index, _)) = self.calls.get(call) else {
            return false;
        };
        if let Item::Tool { output, .. }
        | Item::Edit { output, .. }
        | Item::Plan { output, .. }
        | Item::Subagent { output, .. } = &mut self.items[index]
        {
            *output = Some(out);
        }
        true
    }

    /// Text of an answer: joined to the last item when that is the same
    /// message's text.
    pub fn answer(&mut self, message: &str, id: String, at: Option<u64>, text: &str) {
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

    /// Plan `text` as its item; none when it is the plan shown last (handed
    /// over the same as the file just written).
    pub fn plan(&mut self, id: String, path: Option<String>, text: &str) -> Option<Item> {
        if self.plan.as_deref() == Some(text.trim()) {
            return None;
        }
        self.plan = Some(text.trim().to_owned());
        Some(plan_item(id, path, text))
    }

    /// Whether tool call `call` was read: a subagent it started has a
    /// place to show.
    pub fn knows_call(&self, call: &str) -> bool {
        self.calls.contains_key(call)
    }

    /// The items read so far, in order.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The subagents' items, as their readers have them now.
    pub fn sync_subs(&mut self) {
        for sub in self.subs.iter_mut().filter(|s| s.changed) {
            sub.changed = false;
            if let Item::Subagent { items, output, .. } = &mut self.items[sub.at] {
                *items = Arc::new(sub.reader.items().to_vec());
                // A result that doesn't say how long it took (one run in the
                // background answers at once): its own lines' span.
                if let (Some(out), Some((from, to))) = (output.as_mut(), sub.span)
                    && (out.ms.is_none() || sub.spanned)
                {
                    out.ms = Some(to - from);
                    sub.spanned = true;
                }
            }
        }
    }
}

/// An edit of `path` with its change `diff` when there are both, else a
/// tool with what `tool` gives: its name, gist and input.
pub fn edit_or_tool(
    id: String,
    path: Option<&str>,
    diff: Option<Vec<DiffLine>>,
    tool: impl FnOnce() -> (String, String, String),
) -> Item {
    match (path, diff) {
        (Some(path), Some(diff)) => Item::Edit {
            id,
            path: path.to_owned(),
            diff: capped(diff),
            output: None,
        },
        _ => {
            let (name, gist, input) = tool();
            Item::Tool {
                id,
                name,
                gist,
                input,
                output: None,
            }
        }
    }
}

/// The text between `<name>` and `</name>` in `text`.
pub fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let len = text[start..].find(&format!("</{name}>"))?;
    Some(&text[start..start + len])
}

/// The words of message content: a string, or its text parts; an image as
/// "[image]".
pub fn texts(content: &Value) -> String {
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

/// Plan `text`, written to `path` or not, as its item.
pub fn plan_item(id: String, path: Option<String>, text: &str) -> Item {
    Item::Plan {
        id,
        path,
        text: text.to_owned(),
        blocks: Arc::new(md_blocks::blocks(text)),
        output: None,
    }
}

/// Whether `path` is a plan Claude's plan mode wrote: a markdown file in
/// its plans folder.
pub fn is_plan_file(path: &str) -> bool {
    path.contains("/.claude/plans/") && path.ends_with(".md")
}

/// `old` replaced by `new`, as lines taken out and put in.
pub fn change(old: &str, new: &str) -> Vec<DiffLine> {
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

/// The changes of `edits` one after another, each its text under key `old`
/// replaced by that under `new`.
pub fn flat_edits<'a>(
    edits: impl IntoIterator<Item = &'a Value>,
    old: &str,
    new: &str,
) -> Vec<DiffLine> {
    let said = |e: &'a Value, key: &str| e[key].as_str().unwrap_or("");
    (edits.into_iter())
        .flat_map(|e| change(said(e, old), said(e, new)))
        .collect()
}

/// `text` kept to its first `MAX_LINES` lines and `MAX_BYTES` bytes, saying
/// how many lines it left out.
pub fn cap(text: &str) -> String {
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

/// `diff` kept to its first `MAX_LINES` lines, saying how many it left out.
pub fn capped(mut diff: Vec<DiffLine>) -> Vec<DiffLine> {
    if diff.len() > MAX_LINES {
        let text = more(diff.len() - MAX_LINES);
        diff.truncate(MAX_LINES);
        diff.push(DiffLine {
            side: DiffSide::Context,
            text,
        });
    }
    diff
}

pub fn more(lines: usize) -> String {
    match lines {
        1 => "\u{2026} 1 more line".to_owned(),
        n => format!("\u{2026} {n} more lines"),
    }
}

/// A tool's input as shown: its command, else its JSON.
pub fn input_text(input: &Value) -> String {
    (input["command"].as_str().map(str::to_owned))
        .unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default())
}

/// The one line a tool's `input` says: the first of `keys` it gives a
/// string for, else as Claude's.
pub fn gist(input: &Value, keys: &[&str]) -> String {
    // A string alone `tool_gist` gives as its first line, cut short.
    (keys.iter())
        .find_map(|k| input.get(*k).filter(|v| v.is_string()).and_then(tool_gist))
        .or_else(|| tool_gist(input))
        .unwrap_or_default()
}
