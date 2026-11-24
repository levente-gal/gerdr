//! Claude Code's hooks, for live word (see `super`): `gerdr claude-hook`
//! reads a hook's input and tells gerdr; `merged` adds it to
//! `~/.claude/settings.json`.
//!
//! Wave 2 B fills this in; the signatures are the contract.

/// The subcommand Claude's hooks run.
pub const HOOK_COMMAND: &str = "claude-hook";

/// `gerdr claude-hook`: read the hook's input from stdin and tell gerdr;
/// for `PermissionRequest`, print the chat's answer, if any. Always exits
/// at once without gerdr, and never fails the hook.
pub fn run_hook() {}

/// `existing`, the text of `~/.claude/settings.json`, with `command` run
/// for gerdr's events, in place of any older gerdr's, all else kept.
pub fn merged(existing: Option<&str>, command: &str) -> Result<String, String> {
    let _ = command;
    Ok(existing.unwrap_or("{}").to_owned())
}
