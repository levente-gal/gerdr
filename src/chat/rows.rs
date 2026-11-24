//! A chat's items as they show: the tool steps between two things said
//! folded under one line ("Ran 2 commands, read 1 file"), so the words
//! carry the chat. Display only: the items stay as read.

use std::{collections::HashSet, path::Path};

use super::Item;

/// One row of a chat's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// An item on its own: words, an edit, a subagent, a note.
    Item(usize),
    /// Items `from..to` (tools, thinking) under one line; while `open`,
    /// each follows it as a `Step`.
    Steps { from: usize, to: usize, open: bool },
    /// An item of an open `Steps`, in under its line.
    Step(usize),
    /// A row of the open subagent card at item `sub`, in under it: `row`
    /// is of the subagent's own items.
    In { sub: usize, row: Box<Row> },
}

impl Row {
    /// The items its indexes are into, of `items` or a subagent's down
    /// them, and the row there.
    fn level<'a>(&'a self, items: &'a [Item]) -> (&'a [Item], &'a Row) {
        match self {
            Row::In { sub, row } => match items.get(*sub) {
                Some(Item::Subagent { items, .. }) => row.level(items),
                _ => (&[], row),
            },
            _ => (items, self),
        }
    }

    /// How many subagent cards it is in under.
    pub fn depth(&self) -> usize {
        match self {
            Row::In { row, .. } => 1 + row.depth(),
            _ => 0,
        }
    }

    /// What `unfolded` holds for it while it is open, or its card is.
    pub fn fold_key(&self, items: &[Item]) -> Option<String> {
        let (items, row) = self.level(items);
        match row {
            Row::Item(i) | Row::Step(i) => items.get(*i).map(|item| item.id().to_owned()),
            Row::Steps { from, .. } => items.get(*from).map(|item| steps_key(item.id())),
            Row::In { .. } => None,
        }
    }

    /// The one item it shows, if it is one.
    pub fn item<'a>(&'a self, items: &'a [Item]) -> Option<&'a Item> {
        let (items, row) = self.level(items);
        match row {
            Row::Item(i) | Row::Step(i) => items.get(*i),
            _ => None,
        }
    }

    /// The items it shows: one, or a steps line's.
    pub fn items<'a>(&'a self, items: &'a [Item]) -> &'a [Item] {
        let (items, row) = self.level(items);
        let range = match row {
            Row::Item(i) | Row::Step(i) => *i..*i + 1,
            Row::Steps { from, to, .. } => *from..*to,
            Row::In { .. } => 0..0,
        };
        items.get(range).unwrap_or_default()
    }

    /// Whether it shows the chat's own item `i` (not a subagent's).
    pub fn holds(&self, i: usize) -> bool {
        match self {
            Row::Item(at) | Row::Step(at) => *at == i,
            Row::Steps { from, to, .. } => (*from..*to).contains(&i),
            Row::In { .. } => false,
        }
    }
}

/// What `unfolded` holds for the steps from item `id` on while they show.
pub fn steps_key(id: &str) -> String {
    format!("{id}\u{0}steps")
}

/// Whether `item` folds under a steps line.
fn is_step(item: &Item) -> bool {
    matches!(item, Item::Tool { .. } | Item::Thinking { .. })
}

/// `items` as rows, as `unfolded` has their steps.
pub fn rows(items: &[Item], unfolded: &HashSet<String>) -> Vec<Row> {
    let mut rows = Vec::with_capacity(items.len());
    let mut i = 0;
    while i < items.len() {
        if !is_step(&items[i]) {
            rows.push(Row::Item(i));
            i += 1;
            continue;
        }
        let from = i;
        while i < items.len() && is_step(&items[i]) {
            i += 1;
        }
        let open = unfolded.contains(&steps_key(items[from].id()));
        rows.push(Row::Steps { from, to: i, open });
        if open {
            rows.extend((from..i).map(Row::Step));
        }
    }
    // An open subagent card: its own rows follow it.
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let sub = match row {
            Row::Item(i) => match &items[i] {
                Item::Subagent { id, items: own, .. } if unfolded.contains(id) => Some((i, own)),
                _ => None,
            },
            _ => None,
        };
        out.push(row);
        if let Some((sub, own)) = sub {
            out.extend((self::rows(own, unfolded).into_iter()).map(|row| Row::In {
                sub,
                row: Box::new(row),
            }));
        }
    }
    out
}

/// What a steps line says of its items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// "Read 2 files, ran 3 commands"; "Thought" for thinking alone.
    pub text: String,
    /// How long its tools took, those that said.
    pub ms: u64,
    pub failed: usize,
    /// The gist of a tool still running.
    pub running: Option<String>,
}

/// The verb, and the noun one and many, a tool's work is counted under.
fn counted(name: &str) -> (&'static str, &'static str, &'static str) {
    match name.to_ascii_lowercase().as_str() {
        "read" | "read_file" | "view" | "notebookread" => ("read", "file", "files"),
        "bash" | "shell" | "exec" | "exec_command" | "local_shell" | "local_shell_call"
        | "run_terminal_cmd" | "bashoutput" => ("ran", "command", "commands"),
        "grep" | "glob" | "search" | "grep_search" | "file_search" | "codebase_search" | "find"
        | "rg" => ("searched", "pattern", "patterns"),
        "list" | "ls" | "list_dir" => ("listed", "folder", "folders"),
        "webfetch" | "fetch" | "web_fetch" => ("fetched", "page", "pages"),
        "websearch" | "web_search" | "web_search_call" => ("ran", "web search", "web searches"),
        "task" | "agent" => ("started", "subagent", "subagents"),
        _ => ("used", "tool", "tools"),
    }
}

/// What the steps `items` did, counted.
pub fn summary(items: &[Item]) -> Summary {
    let mut counts: Vec<((&str, &str, &str), usize)> = Vec::new();
    let mut s = Summary::default();
    for item in items {
        let Item::Tool {
            name, gist, output, ..
        } = item
        else {
            continue;
        };
        let key = counted(name);
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
        match output {
            Some(out) => {
                s.ms += out.ms.unwrap_or(0);
                s.failed += usize::from(out.failed);
            }
            None => s.running = Some(gist.clone()),
        }
    }
    let parts: Vec<String> = (counts.iter())
        .map(|((verb, one, many), n)| match n {
            1 => format!("{verb} 1 {one}"),
            n => format!("{verb} {n} {many}"),
        })
        .collect();
    let text = match parts.is_empty() {
        true => "thought".to_owned(),
        false => parts.join(", "),
    };
    let mut chars = text.chars();
    s.text = (chars.next().map(|c| c.to_uppercase().collect::<String>())).unwrap_or_default()
        + chars.as_str();
    s
}

/// `gist` shorter for its line: a leading `cd <dir> && ` dropped, paths
/// in `cwd` relative to it, those in `home` from `~`.
pub fn short_gist(gist: &str, cwd: Option<&Path>, home: Option<&Path>) -> String {
    let mut text = gist.trim();
    if let Some(rest) = text.strip_prefix("cd ")
        && let Some((dir, after)) = rest.split_once(" && ")
        && !dir.trim().contains(' ')
    {
        text = after.trim_start();
    }
    let mut out = text.to_owned();
    if let Some(cwd) = cwd.and_then(Path::to_str).filter(|c| c.len() > 1) {
        out = out.replace(&format!("{cwd}/"), "");
    }
    if let Some(home) = home.and_then(Path::to_str).filter(|h| h.len() > 1) {
        out = out.replace(&format!("{home}/"), "~/");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ToolOut;

    fn said(id: &str) -> Item {
        Item::User {
            id: id.into(),
            at: None,
            text: "hi".into(),
        }
    }

    fn tool(id: &str, name: &str, out: Option<(bool, u64)>) -> Item {
        Item::Tool {
            id: id.into(),
            name: name.into(),
            gist: format!("{name} gist"),
            input: String::new(),
            output: out.map(|(failed, ms)| ToolOut {
                text: String::new(),
                failed,
                ms: Some(ms),
            }),
        }
    }

    fn edit(id: &str) -> Item {
        Item::Edit {
            id: id.into(),
            path: "a.rs".into(),
            diff: Vec::new(),
            output: None,
        }
    }

    #[test]
    fn steps_between_words_fold_under_one_row_and_edits_stand_apart() {
        let items = [
            said("u"),
            tool("t1", "Read", Some((false, 5))),
            Item::Thinking {
                id: "th".into(),
                text: "hm".into(),
            },
            tool("t2", "Bash", Some((false, 5))),
            edit("e"),
            tool("t3", "Bash", Some((false, 5))),
            said("u2"),
        ];
        let folded = rows(&items, &HashSet::new());
        assert_eq!(
            folded,
            [
                Row::Item(0),
                Row::Steps {
                    from: 1,
                    to: 4,
                    open: false
                },
                Row::Item(4),
                Row::Steps {
                    from: 5,
                    to: 6,
                    open: false
                },
                Row::Item(6),
            ]
        );
        let key = folded[1].fold_key(&items).unwrap();
        assert_eq!(key, steps_key("t1"));
        let open = rows(&items, &HashSet::from([key]));
        assert_eq!(
            &open[1..5],
            [
                Row::Steps {
                    from: 1,
                    to: 4,
                    open: true
                },
                Row::Step(1),
                Row::Step(2),
                Row::Step(3),
            ]
        );
        assert_eq!(open[2].fold_key(&items).as_deref(), Some("t1"));
        assert_eq!(open[1].items(&items).len(), 3);
    }

    #[test]
    fn an_open_subagents_own_rows_follow_its_card() {
        let own = vec![
            said("s1"),
            tool("st1", "Bash", Some((false, 5))),
            edit("se"),
        ];
        let items = [
            said("u"),
            Item::Subagent {
                id: "agent:t".into(),
                title: "Look".into(),
                items: std::sync::Arc::new(own),
                output: None,
            },
        ];
        assert_eq!(rows(&items, &HashSet::new()).len(), 2);
        let mut unfolded = HashSet::from(["agent:t".to_owned()]);
        let open = rows(&items, &unfolded);
        let inner = |row| Row::In {
            sub: 1,
            row: Box::new(row),
        };
        let steps = Row::Steps {
            from: 1,
            to: 2,
            open: false,
        };
        assert_eq!(
            open,
            [
                Row::Item(0),
                Row::Item(1),
                inner(Row::Item(0)),
                inner(steps),
                inner(Row::Item(2))
            ]
        );
        // Its rows fold, copy and open as the chat's do.
        assert_eq!(open[3].fold_key(&items), Some(steps_key("st1")));
        assert_eq!(open[4].item(&items).map(Item::id), Some("se"));
        assert_eq!(open[3].items(&items).len(), 1);
        assert_eq!(open[3].depth(), 1);
        assert!(open[1].holds(1) && !open[2].holds(0));
        unfolded.insert(steps_key("st1"));
        let deeper = rows(&items, &unfolded);
        assert_eq!(deeper[4], inner(Row::Step(1)));
    }

    #[test]
    fn a_summary_counts_each_agents_tools_by_what_they_did() {
        let items = [
            tool("a", "Read", Some((false, 400))),
            tool("b", "shell", Some((true, 1200))),
            tool("c", "read", Some((false, 0))),
            tool("d", "exec_command", None),
            tool("e", "Grep", Some((false, 0))),
            tool("f", "todowrite", Some((false, 0))),
        ];
        let s = summary(&items);
        assert_eq!(
            s.text,
            "Read 2 files, ran 2 commands, searched 1 pattern, used 1 tool"
        );
        assert_eq!((s.ms, s.failed), (1600, 1));
        assert_eq!(s.running.as_deref(), Some("exec_command gist"));
        let thought = [Item::Thinking {
            id: "t".into(),
            text: "x".into(),
        }];
        assert_eq!(summary(&thought).text, "Thought");
    }

    #[test]
    fn a_gist_drops_its_cd_and_shortens_its_paths() {
        let (cwd, home) = (Path::new("/home/u/src/app"), Path::new("/home/u"));
        let short = |gist| short_gist(gist, Some(cwd), Some(home));
        assert_eq!(short("cd ~/src/app && cargo test"), "cargo test");
        assert_eq!(short("/home/u/src/app/src/main.rs"), "src/main.rs");
        assert_eq!(short("grep x /home/u/.cache/log"), "grep x ~/.cache/log");
        assert_eq!(short("cd \"a b\" && ls"), "cd \"a b\" && ls");
        assert_eq!(short_gist("/x/y", None, None), "/x/y");
    }
}
