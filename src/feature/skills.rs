//! The skills a stage's agent is told to use: named per stage in the
//! project's `workflow.skills`, else the config file's `[skills]`, else
//! `DEFAULTS`. Only those installed for
//! the agent's CLI are named, and gerdr never copies a skill's text: a
//! skill renamed or gone upstream is simply left out. Each one named is
//! stamped on the run with its folder hash from the `skills` CLI's lock
//! file, so a change in how an agent works can be traced to a skill's
//! update.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::Stage;
use crate::{ai_work::locate::Homes, integrations::Integration};

/// The skills a stage names when its project names none for it.
pub const DEFAULTS: &[(Stage, &[&str])] = &[
    (Stage::Discover, &["grilling", "domain-modeling"]),
    (Stage::Plan, &["codebase-design"]),
    (Stage::Build, &["tdd", "diagnosing-bugs"]),
    (Stage::AiReview, &["codebase-design"]),
];

/// Where "Install skills" gets the skills from when `[skills]` names no
/// `source`: a GitHub repository, as `npx skills add` takes it.
pub const SOURCE: &str = "mattpocock/skills";

/// The config file's `[skills]`: where they are installed from, and the
/// skills each stage names, `DEFAULTS`' for a stage it gives none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub source: String,
    pub stages: BTreeMap<Stage, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::with(None, BTreeMap::new())
    }
}

impl Settings {
    /// `source` (else `SOURCE`), and `stages` over `DEFAULTS`.
    pub fn with(source: Option<String>, stages: BTreeMap<Stage, Vec<String>>) -> Self {
        let mut all: BTreeMap<Stage, Vec<String>> = (DEFAULTS.iter())
            .map(|(stage, names)| (*stage, names.iter().map(|n| (*n).to_owned()).collect()))
            .collect();
        all.extend(stages);
        Self {
            source: source.unwrap_or_else(|| SOURCE.to_owned()),
            stages: all,
        }
    }
}

/// The skills `stage` names: the project's `set` for it where it has an
/// entry (an empty one for none), else `settings`'.
pub fn wanted(
    set: &BTreeMap<Stage, Vec<String>>,
    settings: &Settings,
    stage: Stage,
) -> Vec<String> {
    (set.get(&stage).or_else(|| settings.stages.get(&stage)))
        .cloned()
        .unwrap_or_default()
}

/// Skills a feature's stages name that aren't installed for the agent CLI
/// running them: `agent` by its picker name, `id` the `skills` CLI's name
/// for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub agent: String,
    pub id: &'static str,
    pub names: Vec<String>,
}

/// Of `asks`, each stage's agent (its picker name and command) with the
/// skills it names, those not installed for it in folder `cwd`, by agent,
/// in their order. An agent the `skills` CLI can't install for is left
/// out, and so is a name that isn't a plain folder name.
pub fn missing(cwd: &Path, asks: &[(String, String, Vec<String>)]) -> Vec<Missing> {
    let Some(homes) = Homes::here() else {
        return Vec::new();
    };
    let config = crate::chat::commands::config_dir();
    let mut out: Vec<Missing> = Vec::new();
    for (agent, base, names) in asks {
        let cli = crate::integrations::by_command(base);
        let Some(id) = cli.skills_agent() else {
            continue;
        };
        let found = installed(names, &cli.skill_dirs(cwd, &homes, &config));
        let gone = (names.iter()).filter(|n| plain(n) && !found.contains(n));
        let at = match out.iter().position(|m| m.agent == *agent) {
            Some(at) => at,
            None => {
                out.push(Missing {
                    agent: agent.clone(),
                    id,
                    names: Vec::new(),
                });
                out.len() - 1
            }
        };
        for name in gone {
            if !out[at].names.contains(name) {
                out[at].names.push(name.clone());
            }
        }
    }
    out.retain(|m| !m.names.is_empty());
    out
}

/// What the Discover page says of `missing`: "Not installed for Claude
/// Code: grilling, tdd".
pub fn missing_words(missing: &[Missing]) -> String {
    let each: Vec<String> = (missing.iter())
        .map(|m| format!("{}: {}", m.agent, m.names.join(", ")))
        .collect();
    format!("Not installed for {}", each.join(" \u{b7} "))
}

/// The line that installs `missing` from `source` for every agent of it,
/// for all projects (`-g`), with the `skills` CLI.
pub fn install_line(source: &str, missing: &[Missing]) -> String {
    let mut names: Vec<&str> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for m in missing {
        for name in &m.names {
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
        if !ids.contains(&m.id) {
            ids.push(m.id);
        }
    }
    let names: Vec<String> = names
        .iter()
        .map(|n| crate::desktop::shell_quote(n))
        .collect();
    format!(
        "npx -y skills@latest add {} -g -s {} -a {} -y",
        crate::desktop::shell_quote(source),
        names.join(" "),
        ids.join(" ")
    )
}

/// Those of `names` with a `<name>/SKILL.md` in one of `dirs`, in their
/// order. A name that isn't a plain folder name is left out: it comes from
/// the store file.
pub fn installed(names: &[String], dirs: &[PathBuf]) -> Vec<String> {
    (names.iter())
        .filter(|name| plain(name))
        .filter(|name| {
            dirs.iter()
                .any(|dir| dir.join(name).join("SKILL.md").is_file())
        })
        .cloned()
        .collect()
}

/// Those of `names` installed for `agent` in folder `cwd`, on this machine.
pub fn installed_for(agent: &dyn Integration, cwd: &Path, names: &[String]) -> Vec<String> {
    let Some(homes) = Homes::here() else {
        return Vec::new();
    };
    let config = crate::chat::commands::config_dir();
    installed(names, &agent.skill_dirs(cwd, &homes, &config))
}

/// Whether `name` can be a skill's folder.
pub fn plain(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && (name.chars()).all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// The prompt's list of `names`, one `- \`name\`` line each.
pub fn listed(names: &[String]) -> String {
    (names.iter())
        .map(|name| format!("- `{name}`"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `names` stamped with their folder hashes' first 7 digits from lock file
/// text `lock`, where it has them: ` skills:tdd@667dbdd,pr`. Empty for no
/// names.
pub fn stamp(names: &[String], lock: &str) -> String {
    if names.is_empty() {
        return String::new();
    }
    let lock: serde_json::Value = serde_json::from_str(lock).unwrap_or_default();
    let each: Vec<String> = (names.iter())
        .map(|name| {
            let hash = lock["skills"][name]["skillFolderHash"].as_str();
            match hash.and_then(|h| h.get(..7)) {
                Some(short) => format!("{name}@{short}"),
                None => name.clone(),
            }
        })
        .collect();
    format!(" skills:{}", each.join(","))
}

/// The `skills` CLI's lock file here: `$XDG_STATE_HOME/skills/.skill-lock.json`,
/// else `~/.agents/.skill-lock.json`. Empty when there is none.
pub fn lock_here() -> String {
    let state = (std::env::var_os("XDG_STATE_HOME").map(PathBuf::from))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("skills/.skill-lock.json"));
    let home = crate::home_dir().map(|home| home.join(".agents/.skill-lock.json"));
    (state.into_iter().chain(home))
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(of: &[&str]) -> Vec<String> {
        of.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn a_stage_names_the_projects_skills_else_the_config_files_else_the_defaults() {
        let mut set = BTreeMap::new();
        let config = Settings::with(None, BTreeMap::from([(Stage::Plan, names(&["mine"]))]));
        assert_eq!(config.source, SOURCE);
        assert_eq!(
            wanted(&set, &config, Stage::Build),
            names(&["tdd", "diagnosing-bugs"])
        );
        assert_eq!(wanted(&set, &config, Stage::Plan), names(&["mine"]));
        assert!(wanted(&set, &config, Stage::Verify).is_empty());
        set.insert(Stage::Build, names(&["tests-first"]));
        set.insert(Stage::Discover, Vec::new());
        assert_eq!(wanted(&set, &config, Stage::Build), names(&["tests-first"]));
        // An empty entry names none, over the defaults.
        assert!(wanted(&set, &config, Stage::Discover).is_empty());
    }

    #[test]
    fn a_skill_is_missing_for_an_agent_that_reads_none_of_that_name() {
        let cwd = std::env::temp_dir().join(format!("gerdr-missing-{}", std::process::id()));
        let skill = cwd.join(".claude/skills/tdd");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: tdd\n---\n").unwrap();
        let gone = "gerdr-no-such-skill";
        let asks = [
            ("Claude Code".into(), "claude".into(), names(&["tdd", gone])),
            (
                "Claude Code".into(),
                "claude".into(),
                names(&[gone, "../x"]),
            ),
            // The skills CLI can't install for it: not told.
            ("Mine".into(), "my-agent".into(), names(&[gone])),
        ];
        assert_eq!(
            missing(&cwd, &asks),
            [Missing {
                agent: "Claude Code".into(),
                id: "claude-code",
                names: names(&[gone])
            }]
        );
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn missing_skills_install_in_one_line_for_every_agent_without_them() {
        let missing = [
            Missing {
                agent: "Claude Code".into(),
                id: "claude-code",
                names: names(&["tdd", "grilling"]),
            },
            Missing {
                agent: "Codex".into(),
                id: "codex",
                names: names(&["tdd"]),
            },
        ];
        assert_eq!(
            install_line("mattpocock/skills", &missing),
            "npx -y skills@latest add mattpocock/skills -g -s tdd grilling -a claude-code codex -y"
        );
        assert_eq!(
            missing_words(&missing),
            "Not installed for Claude Code: tdd, grilling \u{b7} Codex: tdd"
        );
    }

    #[test]
    fn only_skills_found_in_a_folder_are_named() {
        let root = std::env::temp_dir().join(format!("gerdr-skills-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        for (dir, skill) in [(&a, "tdd"), (&b, "grilling")] {
            std::fs::create_dir_all(dir.join(skill)).unwrap();
            std::fs::write(dir.join(skill).join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        }
        std::fs::create_dir_all(a.join("empty")).unwrap();
        let found = installed(
            &names(&["grilling", "empty", "gone", "tdd", "../a/tdd", ""]),
            &[a, b],
        );
        assert_eq!(found, names(&["grilling", "tdd"]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_stamp_names_each_skill_with_its_folder_hash() {
        let lock = r#"{"version":3,"skills":{"tdd":{"skillFolderHash":"667dbdd00c11"}}}"#;
        assert_eq!(
            stamp(&names(&["tdd", "pr"]), lock),
            " skills:tdd@667dbdd,pr"
        );
        assert_eq!(stamp(&names(&["tdd"]), "not json"), " skills:tdd");
        assert_eq!(stamp(&[], lock), "");
        assert_eq!(listed(&names(&["tdd", "pr"])), "- `tdd`\n- `pr`");
    }
}
