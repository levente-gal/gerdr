//! OpenCode's slash commands: its built-in ones, and those in its command
//! folders and `opencode.json`.

use std::{fs, path::Path};

use serde_json::Value;

use crate::{
    chat::commands::Command,
    integrations::common::commands::{one_line, tree},
};

// shortcut: built-ins are hard-coded as OpenCode 2.3.0 had them here;
// refresh them when it changes.
pub(super) const BUILTINS: &[(&str, &str)] = &[
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

/// OpenCode's command folders and `opencode.json`, the project's and the
/// config's.
pub(super) fn on_disk(cwd: &Path, config: &Path, found: &mut Vec<Command>) {
    for root in [cwd.join(".opencode"), config.join("opencode")] {
        for dir in ["command", "commands"] {
            tree(&root.join(dir), "", found);
        }
        json(&root.join("opencode.json"), found);
    }
}

/// OpenCode's commands in the `command` key of its config file `path`.
fn json(path: &Path, found: &mut Vec<Command>) {
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
            mention: false,
        });
    }
}
