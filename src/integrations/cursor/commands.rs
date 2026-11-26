//! The Cursor CLI's commands: its built-ins, and on disk its and Claude
//! Code's command files, the project's and the home's, and their skills.

use std::{fs, path::Path};

use crate::{
    ai_work::locate::Homes,
    chat::commands::{Command, md_files, one_line, skills},
};

// shortcut: hard-coded as Cursor 2026.11.22 had them; refresh them when it
// changes.
pub(crate) const CURSOR: &[(&str, &str)] = &[
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

/// Cursor's `.cursor` and `.claude` commands on disk, the project's and the
/// home's, and their skills.
pub(crate) fn cursor_disk(cwd: &Path, homes: &Homes, found: &mut Vec<Command>) {
    let home = homes.cursor.parent().unwrap_or(Path::new("/"));
    for root in [cwd.to_owned(), home.to_owned()] {
        for dir in [".cursor/commands", ".claude/commands"] {
            cursor_commands(&root.join(dir), found);
        }
    }
    for dir in [
        cwd.join(".cursor/skills"),
        cwd.join(".claude/skills"),
        homes.cursor.join("skills"),
        homes.cursor.join("skills-cursor"),
    ] {
        skills(&dir, "", found);
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
