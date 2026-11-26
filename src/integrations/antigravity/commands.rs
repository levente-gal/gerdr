//! The Antigravity CLI's slash commands: its built-in ones, and the skills
//! on disk, its plugins' too.

use std::path::Path;

use crate::{
    ai_work::locate::Homes,
    chat::commands::{Command, named_skills, sub_dirs},
};

// shortcut: built-ins are hard-coded as Antigravity CLI 1.5.15 had them
// here; refresh them when it changes.
pub(super) const BUILTINS: &[(&str, &str)] = &[
    ("add-dir", "Add a working folder"),
    ("agents", "Open the agent manager"),
    ("artifact", "Review the artifacts"),
    ("boost", "Reason deeper with more agents"),
    ("browser", "Hand a task to the browser agent"),
    ("btw", "Ask on the side"),
    ("changelog", "Show the release notes"),
    ("clear", "Start a new conversation"),
    ("codesearch", "Search the code"),
    ("compact", "Compact the conversation"),
    ("config", "Open the settings"),
    ("context", "Show the context use"),
    ("copy", "Copy the last answer"),
    ("credits", "Show the credits left"),
    ("diff", "Show the changes"),
    ("effort", "Set the reasoning effort"),
    ("exit", "Quit"),
    ("feedback", "Send feedback"),
    ("fork", "Fork the conversation"),
    ("goal", "Work until the goal is met"),
    ("grill-me", "Ask me questions first"),
    ("help", "Show commands and shortcuts"),
    ("hooks", "Show the hooks"),
    ("keybindings", "Edit the shortcuts"),
    ("learn", "Keep corrections as rules or skills"),
    ("logout", "Sign out"),
    ("mcp", "Manage MCP servers"),
    ("model", "Pick the model"),
    ("open", "Open a file in the editor"),
    ("permissions", "Set what it may do"),
    ("plan", "Plan first"),
    ("plugin", "Manage plugins"),
    ("remote-control", "Turn remote control on or off"),
    ("rename", "Rename the conversation"),
    ("resume", "Resume a conversation"),
    ("rewind", "Go back to an earlier message"),
    ("schedule", "Wake the agent later"),
    ("skills", "List skills"),
    ("statusline", "Edit the status bar"),
    ("swarm", "Run a swarm of agents"),
    ("tasks", "Show background tasks"),
    ("teamwork", "Run a team of agents"),
    ("title", "Set the window title"),
    ("usage", "Show usage"),
    ("voice", "Talk or dictate"),
];

/// The Antigravity CLI's skills on disk, its plugins' too.
pub(super) fn on_disk(cwd: &Path, homes: &Homes, found: &mut Vec<Command>) {
    // shortcut: every plugin's skills show, enabled or not.
    let cli = &homes.antigravity;
    let gemini = cli.parent().unwrap_or(Path::new("/"));
    let mut dirs = vec![cwd.join(".agents/skills"), cwd.join(".agent/skills")];
    dirs.push(gemini.join("config/skills"));
    dirs.push(cli.join("skills"));
    dirs.push(gemini.join("skills"));
    for plugins in [cwd.join(".agents/plugins"), gemini.join("config/plugins")] {
        dirs.extend(
            sub_dirs(&plugins)
                .into_iter()
                .map(|(_, p)| p.join("skills")),
        );
    }
    dirs.push(cli.join("builtin/skills"));
    let builtin = sub_dirs(&cli.join("builtin/plugins"));
    dirs.extend(builtin.into_iter().map(|(_, p)| p.join("skills")));
    for dir in dirs {
        named_skills(&dir, "", found);
    }
}
