//! The slash commands an agent CLI takes, for the chat's `/` menu: its own
//! built-in ones, and those found on disk (command files, skills, plugins),
//! as each CLI reads them.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::ai_work::{AgentKind, locate::Homes};

/// A command offered: typed as `/<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    /// What it does, in one line; may be empty.
    pub about: String,
    /// What it takes after it (`argument-hint`); may be empty.
    pub hint: String,
}

// shortcut: built-ins are hard-coded as the CLIs were here (Claude Code
// 2.1.331, OpenCode 2.3.0, Cursor 2026.11.22, Codex 0.171.0); refresh them
// when the agents change.
const CLAUDE: &[(&str, &str)] = &[
    ("add-dir", "Add a working folder"),
    ("agents", "Manage agent configurations"),
    ("btw", "Ask a side question"),
    ("clear", "Clear the conversation"),
    ("compact", "Compact the conversation, keeping a summary"),
    ("config", "Open the settings"),
    ("context", "Show what fills the context"),
    ("copy", "Copy the last reply"),
    ("cost", "Show the session's cost"),
    ("diff", "Show the changes made"),
    ("effort", "Set the reasoning effort"),
    ("exit", "Quit"),
    ("export", "Export the conversation"),
    ("fast", "Toggle fast mode"),
    ("feedback", "Send feedback"),
    ("fork", "Fork the conversation"),
    ("help", "Show help"),
    ("hooks", "Manage hooks"),
    ("ide", "Connect to an IDE"),
    ("init", "Write a CLAUDE.md for the project"),
    ("login", "Sign in"),
    ("logout", "Sign out"),
    ("mcp", "Manage MCP servers"),
    ("memory", "Edit memory files"),
    ("model", "Pick the model"),
    ("output-style", "Pick the output style"),
    ("permissions", "Manage permissions"),
    ("plan", "Enter plan mode"),
    ("plugin", "Manage plugins"),
    ("recap", "Recap the session"),
    ("reload-plugins", "Reload plugins"),
    ("rename", "Rename the session"),
    ("resume", "Resume a conversation"),
    ("review", "Review a pull request"),
    ("security-review", "Review the changes for security"),
    ("skills", "List skills"),
    ("status", "Show the status"),
    ("statusline", "Set up the status line"),
    ("tasks", "List background tasks"),
    ("terminal-setup", "Set up terminal keys"),
    ("theme", "Pick the theme"),
    ("usage", "Show plan usage"),
    ("version", "Show the version"),
    ("code-review", "Review the current diff"),
    ("simplify", "Simplify the changed code"),
    ("loop", "Run a prompt on an interval"),
    ("schedule", "Schedule a cloud agent"),
];

const OPENCODE: &[(&str, &str)] = &[
    ("agents", "Pick the agent"),
    ("btw", "Ask a side question"),
    ("cd", "Change the folder"),
    ("clear", "Clear the session"),
    ("compact", "Compact the session"),
    ("connect", "Connect a provider"),
    ("copy", "Copy the session"),
    ("diff", "Show the changes"),
    ("editor", "Write in the editor"),
    ("exit", "Quit"),
    ("export", "Export the session"),
    ("fork", "Fork the session"),
    ("help", "Show help"),
    ("init", "Write AGENTS.md"),
    ("mcps", "List MCP servers"),
    ("models", "Pick the model"),
    ("new", "Start a new session"),
    ("plugins", "List plugins"),
    ("redo", "Redo"),
    ("rename", "Rename the session"),
    ("review", "Review changes"),
    ("sessions", "Pick a session"),
    ("skills", "Pick a skill"),
    ("stats", "Show stats"),
    ("status", "Show the status"),
    ("themes", "Pick the theme"),
    ("timeline", "Jump to a message"),
    ("undo", "Undo the last message"),
    ("usage", "Show usage"),
    ("variants", "Pick the thinking effort"),
];

const CURSOR: &[(&str, &str)] = &[
    ("add-dir", "Add a working folder"),
    ("ask", "Ask without changes"),
    ("btw", "Ask a side question"),
    ("changes", "Show the changes"),
    ("clear", "Start a new chat"),
    ("commit", "Commit the changes"),
    ("config", "Open the settings"),
    ("context", "Show the context"),
    ("copy", "Copy the last reply"),
    ("exit", "Quit"),
    ("fast", "Toggle fast mode"),
    ("feedback", "Send feedback"),
    ("fork", "Fork the chat"),
    ("help", "Show help"),
    ("logout", "Sign out"),
    ("max-mode", "Toggle max mode"),
    ("mcp", "Manage MCP servers"),
    ("model", "Pick the model"),
    ("plan", "Plan first"),
    ("rename", "Rename the chat"),
    ("resume", "Resume a chat"),
    ("rewind", "Go back to a message"),
    ("rule", "Add a rule"),
    ("skills", "List skills"),
    ("summarize", "Summarize the chat"),
    ("usage", "Show usage"),
    ("vim", "Toggle vim keys"),
];

const CODEX: &[(&str, &str)] = &[
    ("compact", "Compact the conversation"),
    ("diff", "Show the changes"),
    ("exit", "Quit"),
    ("fork", "Fork the conversation"),
    ("init", "Write AGENTS.md"),
    ("mcp", "List MCP tools"),
    ("mention", "Mention a file"),
    ("model", "Pick the model"),
    ("new", "Start a new conversation"),
    ("permissions", "Set what it may do"),
    ("plan", "Plan first"),
    ("rename", "Rename the conversation"),
    ("resume", "Resume a conversation"),
    ("review", "Review the changes"),
    ("skills", "List skills"),
    ("status", "Show the status"),
    ("usage", "Show usage"),
];

/// The commands agent `kind` takes in folder `cwd`, by name: the project's
/// before the home's, those on disk before the built-ins. Unreadable files
/// are left out.
pub fn list(kind: AgentKind, cwd: &Path, homes: &Homes, config: &Path) -> Vec<Command> {
    let mut found = Vec::new();
    let builtin = match kind {
        AgentKind::Claude => {
            claude_commands(&cwd.join(".claude/commands"), &mut found);
            skills(&cwd.join(".claude/skills"), "", &mut found);
            for home in &homes.claude {
                claude_commands(&home.join("commands"), &mut found);
                skills(&home.join("skills"), "", &mut found);
                synced(&home.join("skills/synced"), &mut found);
                plugins(home, &mut found);
            }
            CLAUDE
        }
        AgentKind::OpenCode => {
            for root in [cwd.join(".opencode"), config.join("opencode")] {
                for dir in ["command", "commands"] {
                    tree(&root.join(dir), "", &mut found);
                }
                opencode_json(&root.join("opencode.json"), &mut found);
            }
            OPENCODE
        }
        AgentKind::Cursor => {
            let home = homes.cursor.parent().unwrap_or(Path::new("/"));
            for root in [cwd.to_owned(), home.to_owned()] {
                for dir in [".cursor/commands", ".claude/commands"] {
                    cursor_commands(&root.join(dir), &mut found);
                }
            }
            for dir in [
                cwd.join(".cursor/skills"),
                cwd.join(".claude/skills"),
                homes.cursor.join("skills"),
                homes.cursor.join("skills-cursor"),
            ] {
                skills(&dir, "", &mut found);
            }
            CURSOR
        }
        AgentKind::Codex => {
            for (name, path) in md_files(&homes.codex.join("prompts")) {
                found.push(read_md(&path, format!("prompts:{name}")));
            }
            CODEX
        }
        // shortcut: no list for Pi and Antigravity yet; add theirs when
        // someone uses them here.
        AgentKind::Pi | AgentKind::Antigravity | AgentKind::T3 | AgentKind::Unknown => {
            return Vec::new();
        }
    };
    found.extend(builtin.iter().map(|(name, about)| Command {
        name: (*name).to_owned(),
        about: (*about).to_owned(),
        hint: String::new(),
    }));
    let mut seen = std::collections::HashSet::new();
    found.retain(|c| seen.insert(c.name.clone()));
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The commands agent `kind` takes in folder `cwd`, on this machine.
pub fn here(kind: AgentKind, cwd: &Path) -> Vec<Command> {
    let Some(homes) = Homes::here() else {
        return Vec::new();
    };
    let config = (std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .filter(|dir| dir.is_absolute())
        .or_else(|| crate::home_dir().map(|home| home.join(".config")))
        .unwrap_or_default();
    list(kind, cwd, &homes, &config)
}

/// What follows the `/` of a reply that is a command being typed: a `/`
/// and no space yet.
pub fn query(draft: &str) -> Option<&str> {
    let typed = draft.strip_prefix('/')?;
    (!typed.chars().any(char::is_whitespace)).then_some(typed)
}

/// How many of the menu's rows show at once; the picked one always does.
pub const ROWS: usize = 8;

/// The `/` menu over the reply box: the commands its agent takes, those
/// matching what follows the `/`, best first.
#[derive(Debug, Default)]
pub struct Slash {
    pub all: Vec<Command>,
    /// What it was filtered for; none before the first time.
    query: Option<String>,
    /// The commands matching, by index in `all`.
    pub shown: Vec<usize>,
    pub picked: usize,
    /// Esc closed it; typing on opens it again.
    pub hidden: bool,
}

impl Slash {
    pub fn new(all: Vec<Command>) -> Self {
        Self {
            all,
            ..Self::default()
        }
    }

    /// Whether it shows, and takes its keys.
    pub fn open(&self) -> bool {
        !self.hidden && !self.shown.is_empty()
    }

    /// Filter it for `query`, the best match picked; the same query again
    /// changes nothing.
    pub fn sync(&mut self, query: &str) {
        if self.query.as_deref() == Some(query) {
            return;
        }
        let all = &self.all;
        let ranked = crate::fuzzy::rank(query, 0..all.len(), |i| all[*i].name.clone(), |_| 0.0);
        self.shown = ranked.into_iter().map(|r| r.item).collect();
        self.picked = 0;
        self.hidden = false;
        self.query = Some(query.to_owned());
    }

    /// Pick the row `by` rows down (up when negative), within the rows.
    pub fn step(&mut self, by: isize) {
        let last = self.shown.len().saturating_sub(1);
        self.picked = self.picked.saturating_add_signed(by).min(last);
    }

    /// The command picked.
    pub fn picked(&self) -> Option<&Command> {
        self.shown.get(self.picked).map(|i| &self.all[*i])
    }

    /// The rows in sight, from the first of them: at most `ROWS`, the
    /// picked one among them.
    pub fn in_sight(&self) -> (usize, &[usize]) {
        let first = (self.picked + 1).saturating_sub(ROWS);
        (first, &self.shown[first..(first + ROWS).min(self.shown.len())])
    }
}

/// The `*.md` files right in `dir`, by name without `.md`, in name order.
fn md_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.to_owned(), p)))
        .collect();
    files.sort();
    files
}

/// The folders right in `dir`, by name, in name order.
fn sub_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| Some((p.file_name()?.to_str()?.to_owned(), p)))
        .collect();
    dirs.sort();
    dirs
}

/// Claude Code's command files in `dir`: those in a subfolder `ns` are
/// `ns:name`.
fn claude_commands(dir: &Path, found: &mut Vec<Command>) {
    tree(dir, "", found);
    for (ns, sub) in sub_dirs(dir) {
        tree(&sub, &format!("{ns}:"), found);
    }
}

/// The `*.md` files in `dir` as commands named `<prefix><name>`.
fn tree(dir: &Path, prefix: &str, found: &mut Vec<Command>) {
    for (name, path) in md_files(dir) {
        found.push(read_md(&path, format!("{prefix}{name}")));
    }
}

/// The skills in `dir` (`<name>/SKILL.md`) as commands `<prefix><name>`.
fn skills(dir: &Path, prefix: &str, found: &mut Vec<Command>) {
    for (name, sub) in sub_dirs(dir) {
        let path = sub.join("SKILL.md");
        if path.is_file() {
            found.push(read_md(&path, format!("{prefix}{name}")));
        }
    }
}

/// Claude Code's skills synced from claude.ai: `<id>/<name>/SKILL.md`.
fn synced(dir: &Path, found: &mut Vec<Command>) {
    for (_, sub) in sub_dirs(dir) {
        skills(&sub, "anthropic-skills:", found);
    }
}

/// The commands and skills of Claude Code's plugins enabled in `home`'s
/// settings, as `<plugin>:<name>`.
fn plugins(home: &Path, found: &mut Vec<Command>) {
    let json = |path: PathBuf| -> Value {
        (fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    };
    let enabled = json(home.join("settings.json"));
    let installed = json(home.join("plugins/installed_plugins.json"));
    let Some(installed) = installed["plugins"].as_object() else {
        return;
    };
    for (id, installs) in installed {
        if enabled["enabledPlugins"][id] != Value::Bool(true) {
            continue;
        }
        let Some(path) = installs[0]["installPath"].as_str() else {
            continue;
        };
        let plugin = id.split('@').next().unwrap_or(id);
        let root = Path::new(path);
        tree(&root.join("commands"), &format!("{plugin}:"), found);
        skills(&root.join("skills"), &format!("{plugin}:"), found);
    }
}

/// OpenCode's commands in the `command` key of its config file `path`.
fn opencode_json(path: &Path, found: &mut Vec<Command>) {
    // shortcut: opencode.jsonc (comments) isn't read; parse it once someone
    // keeps commands there.
    let config: Value = (fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let Some(commands) = config["command"].as_object() else {
        return;
    };
    for (name, command) in commands {
        found.push(Command {
            name: name.clone(),
            about: one_line(command["description"].as_str().unwrap_or("")),
            hint: String::new(),
        });
    }
}

/// Cursor's command files in `dir`: their first line says what they do (it
/// reads no front matter).
fn cursor_commands(dir: &Path, found: &mut Vec<Command>) {
    for (name, path) in md_files(dir) {
        let text = fs::read_to_string(&path).unwrap_or_default();
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        found.push(Command {
            name,
            about: one_line(first.trim_start_matches('#')),
            hint: String::new(),
        });
    }
}

/// Command `name` from markdown file `path`, with what its front matter
/// says: `description` and `argument-hint`.
fn read_md(path: &Path, name: String) -> Command {
    let text = fs::read_to_string(path).unwrap_or_default();
    let front = front_matter(&text);
    let field = |key: &str| {
        front
            .get(key)
            .and_then(|v| v.as_str())
            .map(one_line)
            .unwrap_or_default()
    };
    Command {
        name,
        about: field("description"),
        hint: field("argument-hint"),
    }
}

/// The YAML front matter at the top of `text`, as text fields by key.
fn front_matter(text: &str) -> HashMap<String, serde_norway::Value> {
    let Some(rest) = text.strip_prefix("---") else {
        return HashMap::new();
    };
    let Some(end) = rest.find("\n---") else {
        return HashMap::new();
    };
    serde_norway::from_str(&rest[..end]).unwrap_or_default()
}

/// `text` on one line, its runs of whitespace one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-cmds-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn names(list: &[Command]) -> Vec<&str> {
        list.iter().map(|c| c.name.as_str()).collect()
    }

    fn find<'a>(list: &'a [Command], name: &str) -> &'a Command {
        list.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn claude_reads_commands_skills_and_enabled_plugins() {
        let tmp = scratch("claude");
        let (home, cwd) = (tmp.join("home"), tmp.join("work"));
        let homes = Homes::under(&home);
        let claude = home.join(".claude");
        write(
            &cwd.join(".claude/commands/ship.md"),
            "---\ndescription: >\n  Ship it\n  now\nargument-hint: \"[pr]\"\n---\nbody",
        );
        write(&claude.join("commands/ship.md"), "---\ndescription: home one\n---\n");
        write(&claude.join("commands/git/push.md"), "no front matter");
        write(&claude.join("skills/graph/SKILL.md"), "---\nname: graph\ndescription: Graphs\n---\n");
        write(&claude.join("skills/synced/abc/docx/SKILL.md"), "---\ndescription: Word\n---\n");
        let (on, off) = (tmp.join("on"), tmp.join("off"));
        write(&on.join("commands/init.md"), "---\ndescription: Init it\n---\n");
        write(&on.join("skills/review/SKILL.md"), "---\ndescription: Review\n---\n");
        write(&off.join("commands/gone.md"), "");
        let plugin = |path: &Path| json!([{ "installPath": path }]);
        write(
            &claude.join("plugins/installed_plugins.json"),
            &json!({ "plugins": { "cave@m": plugin(&on), "off@m": plugin(&off) } }).to_string(),
        );
        write(
            &claude.join("settings.json"),
            &json!({ "enabledPlugins": { "cave@m": true, "off@m": false } }).to_string(),
        );

        let list = list(AgentKind::Claude, &cwd, &homes, &home.join(".config"));
        let ship = find(&list, "ship");
        assert_eq!((ship.about.as_str(), ship.hint.as_str()), ("Ship it now", "[pr]"));
        assert_eq!(find(&list, "git:push").about, "");
        assert_eq!(find(&list, "graph").about, "Graphs");
        assert_eq!(find(&list, "anthropic-skills:docx").about, "Word");
        assert_eq!(find(&list, "cave:init").about, "Init it");
        assert_eq!(find(&list, "cave:review").about, "Review");
        assert!(!names(&list).contains(&"off:gone"));
        assert!(!names(&list).contains(&"synced"));
        // A built-in, once, and all in name order.
        assert_eq!(names(&list).iter().filter(|n| **n == "compact").count(), 1);
        assert!(names(&list).is_sorted());
    }

    #[test]
    fn a_command_file_wins_over_a_builtin_of_its_name() {
        let tmp = scratch("builtin");
        let homes = Homes::under(&tmp);
        write(&tmp.join("w/.claude/commands/review.md"), "---\ndescription: Mine\n---\n");
        let list = list(AgentKind::Claude, &tmp.join("w"), &homes, &tmp);
        assert_eq!(find(&list, "review").about, "Mine");
    }

    #[test]
    fn opencode_cursor_and_codex_read_their_own_places() {
        let tmp = scratch("others");
        let (home, cwd) = (tmp.join("home"), tmp.join("work"));
        let homes = Homes::under(&home);
        let config = home.join(".config");
        write(&config.join("opencode/commands/tidy.md"), "---\ndescription: Tidy up\n---\n");
        write(&cwd.join(".opencode/command/ci/fix.md"), "");
        write(
            &config.join("opencode/opencode.json"),
            r#"{"command": {"test": {"template": "run", "description": "Run tests"}}}"#,
        );
        let open = list(AgentKind::OpenCode, &cwd, &homes, &config);
        assert_eq!(find(&open, "tidy").about, "Tidy up");
        assert_eq!(find(&open, "test").about, "Run tests");
        assert!(find(&open, "models").about.contains("model"));
        // Only files right in the folder.
        assert!(!names(&open).iter().any(|n| n.contains("fix")));

        write(&cwd.join(".cursor/commands/lint.md"), "# Lint the code\n\nmore");
        write(&home.join(".cursor/skills-cursor/create-rule/SKILL.md"), "---\ndescription: Rule\n---\n");
        let cursor = list(AgentKind::Cursor, &cwd, &homes, &config);
        assert_eq!(find(&cursor, "lint").about, "Lint the code");
        assert_eq!(find(&cursor, "create-rule").about, "Rule");

        write(&home.join(".codex/prompts/draft.md"), "---\ndescription: Draft\n---\n");
        let codex = list(AgentKind::Codex, &cwd, &homes, &config);
        assert_eq!(find(&codex, "prompts:draft").about, "Draft");

        assert!(list(AgentKind::Unknown, &cwd, &homes, &config).is_empty());
    }

    fn cmd(name: &str) -> Command {
        Command {
            name: name.into(),
            about: String::new(),
            hint: String::new(),
        }
    }

    #[test]
    fn the_menu_filters_picks_and_keeps_the_picked_row_in_sight() {
        assert_eq!(query("/co"), Some("co"));
        assert_eq!(query("/"), Some(""));
        assert_eq!(query("/compact now"), None);
        assert_eq!(query("hi /co"), None);

        let mut slash = Slash::new(["clear", "compact", "config", "cost"].map(cmd).to_vec());
        slash.sync("");
        assert_eq!(slash.shown.len(), 4);
        slash.sync("cmp");
        assert_eq!(slash.picked().unwrap().name, "compact");
        slash.step(5);
        assert_eq!(slash.picked, 0);
        slash.sync("xyz");
        assert!(!slash.open());

        let many: Vec<Command> = (0..20).map(|i| cmd(&format!("c{i:02}"))).collect();
        let mut slash = Slash::new(many);
        slash.sync("");
        slash.step(10);
        let (first, rows) = slash.in_sight();
        assert_eq!((first, rows.len()), (3, ROWS));
        slash.step(-20);
        assert_eq!(slash.in_sight().0, 0);
        slash.hidden = true;
        slash.sync("");
        assert!(!slash.open(), "the same query keeps it closed");
        slash.sync("c1");
        assert!(slash.open(), "typing on opens it again");
    }
}
