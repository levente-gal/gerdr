//! The agents a workspace can start (see `Gerdr::start_agent`): the known
//! agent CLIs that are installed, as the `[agents]` table of the config
//! file changes them, the ones it adds, and T3 Code once it ran here,
//! whose threads start in its app in a web pane instead of in a terminal.

use std::collections::BTreeMap;

use crate::{agent_icons::AgentIcon, desktop};

/// An agent to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliAgent {
    /// What the picker calls it ("Claude Code").
    pub name: String,
    pub start: Start,
    pub icon: AgentIcon,
}

/// How an agent starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// By this command, run by `sh` in the workspace folder in a new tab.
    Command(String),
    /// As a new thread of T3 Code's app, in the workspace's web pane
    /// showing it (see `Gerdr::new_t3_thread`).
    T3,
}

impl CliAgent {
    /// The command it runs, for a CLI.
    pub fn command(&self) -> Option<&str> {
        match &self.start {
            Start::Command(command) => Some(command),
            Start::T3 => None,
        }
    }
}

/// The `[agents]` key of T3 Code, which isn't a command: `""` hides it,
/// another value is ignored.
const T3: &str = "t3";

/// The agents gerdr knows, in the order offered: (the `[agents]` key and,
/// but for T3 Code, command, the name).
const KNOWN: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    (T3, "T3 Code"),
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

/// The agents installed: the known ones with `config`'s commands for them
/// (an empty one hides it), T3 Code if it ran here, then the ones `config`
/// adds, by name.
pub fn installed(config: &BTreeMap<String, String>) -> Vec<CliAgent> {
    let t3 = crate::t3_agents::home().is_some();
    offered(config, desktop::installed, t3)
}

/// `installed`, with `installed` telling whether a program is and `t3`
/// whether T3 Code ran here.
fn offered(
    config: &BTreeMap<String, String>,
    installed: impl Fn(&str) -> bool,
    t3: bool,
) -> Vec<CliAgent> {
    let known = KNOWN.iter().filter_map(|(key, name)| {
        let set = config.get(*key).map(|c| c.trim());
        let start = if *key == T3 {
            (t3 && set != Some("")).then_some(Start::T3)?
        } else {
            let command = set.unwrap_or(key);
            let on_path = program(command).is_some_and(&installed);
            on_path.then(|| Start::Command(command.to_owned()))?
        };
        Some(agent(key, (*name).to_owned(), start))
    });
    let added = config
        .iter()
        .filter(|(key, _)| !KNOWN.iter().any(|(k, _)| k == key))
        .filter(|(_, command)| program(command).is_some_and(&installed))
        .map(|(key, command)| {
            let start = Start::Command(command.trim().to_owned());
            agent(key, key.clone(), start)
        });
    known.chain(added).collect()
}

/// Whether the saved `command` runs an agent CLI: one gerdr knows, installed
/// or not, or one `config` starts.
pub fn runs_agent(command: &str, config: &BTreeMap<String, String>) -> bool {
    use crate::projects::command_program;
    let Some(program) = command_program(command) else {
        return false;
    };
    KNOWN.iter().any(|(key, _)| *key == program)
        || config
            .values()
            .any(|c| command_program(c).as_ref() == Some(&program))
}

/// The agent `[agents]` calls `key`, named `name`, started by `start`.
fn agent(key: &str, name: String, start: Start) -> CliAgent {
    CliAgent {
        icon: AgentIcon::from_agent(key),
        name,
        start,
    }
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

    #[test]
    fn a_saved_command_line_tells_agents() {
        let config = config(&[
            ("mine", "bash ./my-agent.sh --fast"),
            ("codex", ""),
            ("wrapped", "env FOO=1 /opt/bot"),
        ]);
        for agent in [
            "claude",
            "claude --resume 062f528e",
            "/home/me/.local/bin/cursor-agent --trust",
            "node /usr/lib/node_modules/.bin/codex",
            "python /venv/bin/aider",
            "bash ./my-agent.sh",
            "/opt/bot --x",
        ] {
            assert!(runs_agent(agent, &config), "{agent}");
        }
        for service in [
            "bash ./runlocal.sh",
            "bash ./run_celery.sh",
            "python /venv/bin/celery -A app worker",
            "npm run dev",
            "python3",
            "",
        ] {
            assert!(!runs_agent(service, &config), "{service}");
        }
    }

    fn commands(agents: &[CliAgent]) -> Vec<&str> {
        agents.iter().map(|a| a.command().unwrap_or("T3")).collect()
    }

    #[test]
    fn only_the_installed_known_agents_show_in_order() {
        let on_path = |p: &str| ["opencode", "claude", "cursor-agent"].contains(&p);
        let agents = offered(&BTreeMap::new(), on_path, false);
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
        let agents = offered(&config, on_path, false);
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
    fn t3_code_shows_once_it_ran_and_only_its_empty_entry_hides_it() {
        let on_path = |p: &str| ["claude", "opencode"].contains(&p);
        let agents = offered(&BTreeMap::new(), on_path, true);
        assert_eq!(commands(&agents), ["claude", "T3", "opencode"]);
        assert_eq!(agents[1].name, "T3 Code");
        assert_eq!(agents[1].start, Start::T3);
        assert_eq!(agents[1].icon, AgentIcon::T3);
        // A command for it is no command: it still opens the app.
        let set = config(&[("t3", "t3 serve")]);
        assert_eq!(commands(&offered(&set, on_path, true))[1], "T3");
        let hidden = config(&[("t3", " ")]);
        assert_eq!(
            commands(&offered(&hidden, on_path, true)),
            ["claude", "opencode"]
        );
    }

    #[test]
    fn the_program_skips_what_sets_the_environment() {
        assert_eq!(program("env A=1 exec 'claude' -c"), Some("claude"));
        assert_eq!(program("  "), None);
    }
}
