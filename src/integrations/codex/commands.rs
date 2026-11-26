//! Codex's commands: its built-ins, and on disk its `prompts/` as
//! `prompts:<name>`.

use crate::{
    ai_work::locate::Homes,
    chat::commands::{Command, md_files, read_md},
};

// shortcut: hard-coded as Codex 0.171.0 had them; refresh them when it
// changes.
pub(crate) const CODEX: &[(&str, &str)] = &[
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

/// Codex's commands on disk: its `prompts/`.
pub(crate) fn codex_disk(homes: &Homes, found: &mut Vec<Command>) {
    for (name, path) in md_files(&homes.codex.join("prompts")) {
        found.push(read_md(&path, format!("prompts:{name}")));
    }
}
