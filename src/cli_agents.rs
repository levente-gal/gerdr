//! The agent CLIs a workspace can start in a new tab (see
//! `Gerdr::new_agent`): the known ones that are installed, as the
//! `[agents]` table of the config file changes them, and the ones it adds.

use std::collections::BTreeMap;

use crate::{agent_icons::AgentIcon, desktop};

/// An agent CLI to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliAgent {
    /// What the picker calls it ("Claude Code").
    pub name: String,
    /// What runs it, by `sh` in the workspace folder.
    pub command: String,
    pub icon: AgentIcon,
}

/// The agent CLIs gerdr knows, in the order offered: (the `[agents]` key
/// and command, the name).
const KNOWN: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("cursor-agent", "Cursor"),
    ("opencode", "OpenCode"),
    ("copilot", "Copilot"),
    ("amp", "Amp"),
    ("qwen", "Qwen Code"),
    ("kimi", "Kimi"),
    ("grok", "Grok"),
    ("kiro-cli", "Kiro"),
    ("cline", "Cline"),
    ("mastracode", "Mastra"),
    ("droid", "Droid"),
    ("aider", "Aider"),
    ("goose", "Goose"),
    ("crush", "Crush"),
];

/// The agent CLIs installed: the known ones with `config`'s commands for
/// them (an empty one hides it), then the ones `config` adds, by name.
pub fn installed(config: &BTreeMap<String, String>) -> Vec<CliAgent> {
    offered(config, desktop::installed)
}

/// `installed`, with `installed` telling whether a program is.
fn offered(config: &BTreeMap<String, String>, installed: impl Fn(&str) -> bool) -> Vec<CliAgent> {
    let known = KNOWN.iter().map(|(key, name)| {
        let command = config.get(*key).map_or(*key, |c| c.trim());
        (*key, (*name).to_owned(), command)
    });
    let added = config
        .iter()
        .filter(|(key, _)| !KNOWN.iter().any(|(k, _)| k == key))
        .map(|(key, command)| (key.as_str(), key.clone(), command.trim()));
    known
        .chain(added)
        .filter(|(_, _, command)| program(command).is_some_and(&installed))
        .map(|(key, name, command)| CliAgent {
            icon: AgentIcon::from_agent(key),
            name,
            command: command.to_owned(),
        })
        .collect()
}

/// The program `command` runs, as written: past `env`, `exec` and the
/// variables it sets.
fn program(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|word| !matches!(*word, "exec" | "env") && !word.contains('='))
        .map(|word| word.trim_matches(['\'', '"']))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn commands(agents: &[CliAgent]) -> Vec<&str> {
        agents.iter().map(|a| a.command.as_str()).collect()
    }

    #[test]
    fn only_the_installed_known_agents_show_in_order() {
        let on_path = |p: &str| ["opencode", "claude", "cursor-agent"].contains(&p);
        let agents = offered(&BTreeMap::new(), on_path);
        assert_eq!(commands(&agents), ["claude", "cursor-agent", "opencode"]);
        assert_eq!(agents[0].name, "Claude Code");
        assert_eq!(agents[0].icon, AgentIcon::Claude);
        assert_eq!(agents[1].icon, AgentIcon::Cursor);
    }

    #[test]
    fn the_config_changes_hides_and_adds_agents() {
        let config = config(&[
            ("claude", " claude --dangerously-skip-permissions "),
            ("opencode", ""),
            ("my-agent", "FOO=1 my-agent --fast"),
            ("missing", "not-installed"),
        ]);
        let on_path = |p: &str| ["claude", "opencode", "my-agent"].contains(&p);
        let agents = offered(&config, on_path);
        assert_eq!(
            commands(&agents),
            [
                "claude --dangerously-skip-permissions",
                "FOO=1 my-agent --fast"
            ]
        );
        assert_eq!(agents[1].name, "my-agent");
        assert_eq!(agents[1].icon, AgentIcon::Generic);
    }

    #[test]
    fn the_program_skips_what_sets_the_environment() {
        assert_eq!(program("env A=1 exec 'claude' -c"), Some("claude"));
        assert_eq!(program("  "), None);
    }
}
