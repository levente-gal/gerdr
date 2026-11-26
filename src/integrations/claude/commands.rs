//! Claude Code's slash commands: its built-ins, and its command files,
//! skills and plugins on disk.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    ai_work::locate::Homes,
    chat::commands::{Command, skills, sub_dirs, tree},
};

// shortcut: built-ins are hard-coded as Claude Code 2.1.331 had them here;
// refresh them when it changes.
pub(crate) const BUILTINS: &[(&str, &str)] = &[
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

/// Claude Code's commands and skills on disk, the project's and each config
/// folder's, with its synced skills and plugins.
pub(crate) fn on_disk(cwd: &Path, homes: &Homes, found: &mut Vec<Command>) {
    command_files(&cwd.join(".claude/commands"), found);
    skills(&cwd.join(".claude/skills"), "", found);
    for home in &homes.claude {
        command_files(&home.join("commands"), found);
        skills(&home.join("skills"), "", found);
        synced(&home.join("skills/synced"), found);
        plugins(home, found);
    }
}

/// Claude Code's command files in `dir`: those in a subfolder `ns` are
/// `ns:name`.
fn command_files(dir: &Path, found: &mut Vec<Command>) {
    tree(dir, "", found);
    for (ns, sub) in sub_dirs(dir) {
        tree(&sub, &format!("{ns}:"), found);
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use crate::{
        ai_work::{AgentKind, locate::Homes},
        chat::commands::{
            list,
            tests::{find, names, scratch, write},
        },
    };

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
        write(
            &claude.join("commands/ship.md"),
            "---\ndescription: home one\n---\n",
        );
        write(&claude.join("commands/git/push.md"), "no front matter");
        write(
            &claude.join("skills/graph/SKILL.md"),
            "---\nname: graph\ndescription: Graphs\n---\n",
        );
        write(
            &claude.join("skills/synced/abc/docx/SKILL.md"),
            "---\ndescription: Word\n---\n",
        );
        let (on, off) = (tmp.join("on"), tmp.join("off"));
        write(
            &on.join("commands/init.md"),
            "---\ndescription: Init it\n---\n",
        );
        write(
            &on.join("skills/review/SKILL.md"),
            "---\ndescription: Review\n---\n",
        );
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
        assert_eq!(
            (ship.about.as_str(), ship.hint.as_str()),
            ("Ship it now", "[pr]")
        );
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
}
