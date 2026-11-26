//! pi's slash commands: its built-ins, and its prompt templates and skills
//! on disk.

use std::{fs, path::Path};

use crate::{
    ai_work::locate::Homes,
    chat::commands::{Command, md_files, named_skills, one_line, read_md},
};

// shortcut: built-ins are hard-coded as pi 1.6.0 had them here; refresh them
// when it changes.
pub(crate) const BUILTINS: &[(&str, &str)] = &[
    ("bug", "Write a bug report"),
    ("changelog", "Show the changelog"),
    ("clone", "Copy the session"),
    ("compact", "Compact the context"),
    ("copy", "Copy the last answer"),
    ("export", "Export the session"),
    ("fork", "Start a session from an earlier message"),
    ("hotkeys", "Show the shortcuts"),
    ("import", "Import a session"),
    ("llama", "Manage llama.cpp models"),
    ("login", "Sign in to a provider"),
    ("logout", "Sign out of a provider"),
    ("mcp", "Manage MCP servers"),
    ("model", "Pick the model"),
    ("name", "Name the session"),
    ("new", "Start a new session"),
    ("quit", "Quit"),
    ("reload", "Reload extensions, skills and templates"),
    ("resume", "Resume a session"),
    ("scoped-models", "Pick the models to cycle"),
    ("session", "Show the session"),
    ("settings", "Open the settings"),
    ("share", "Share the session"),
    ("thinking", "Set the thinking level"),
    ("tree", "Go through the session tree"),
    ("trust", "Trust this project"),
];

/// pi's prompts and skills on disk.
pub(crate) fn on_disk(cwd: &Path, homes: &Homes, found: &mut Vec<Command>) {
    // shortcut: commands that extensions add are left out (they are
    // code); add them if pi ever lists them on disk.
    let agent = homes.pi.parent().unwrap_or(Path::new("/"));
    let home = homes.cursor.parent();
    for dir in [cwd.join(".pi/prompts"), agent.join("prompts")] {
        prompts(&dir, found);
    }
    let mut dirs = vec![cwd.join(".pi/skills"), cwd.join(".agents/skills")];
    dirs.push(agent.join("skills"));
    dirs.extend(home.map(|home| home.join(".agents/skills")));
    for dir in dirs {
        named_skills(&dir, "skill:", found);
    }
}

/// pi's prompt templates in `dir`: with no `description`, the first line
/// after the front matter says what they do.
fn prompts(dir: &Path, found: &mut Vec<Command>) {
    for (name, path) in md_files(dir) {
        let mut command = read_md(&path, name);
        if command.about.is_empty() {
            let text = fs::read_to_string(&path).unwrap_or_default();
            let body = (text.strip_prefix("---"))
                .and_then(|rest| rest.split_once("\n---"))
                .map_or(text.as_str(), |(_, body)| body);
            let first = body.lines().map(str::trim).find(|l| !l.is_empty());
            command.about = one_line(first.unwrap_or(""));
        }
        found.push(command);
    }
}
