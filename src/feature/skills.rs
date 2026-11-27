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

/// Another machine a stage's agent runs on, as the skills check reaches
/// it: its id and name, and its link's SSH target and master socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub id: String,
    pub name: String,
    pub target: String,
    pub control: PathBuf,
}

/// A stage's agent, for the skills check: its picker name and command,
/// the skills it names, and where it runs when that isn't here, with the
/// home, config folder and feature folder it has there.
#[derive(Clone, Debug)]
pub struct Ask {
    pub agent: String,
    pub base: String,
    pub names: Vec<String>,
    pub there: Option<(Remote, Folders)>,
}

/// The folders of another machine its skills are read under.
#[derive(Clone, Debug)]
pub struct Folders {
    pub home: String,
    pub config: String,
    pub cwd: String,
}

/// Skills a feature's stages name that aren't installed for the agent CLI
/// running them: `agent` by its picker name, `id` the `skills` CLI's name
/// for it, and the machine it runs on when that isn't this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub agent: String,
    pub id: &'static str,
    pub names: Vec<String>,
    pub there: Option<Remote>,
}

/// What the skills check found: those `missing`, and, by machine id and
/// the `skills` CLI's name for the agent, those installed on other
/// machines (their stages' prompts name only those).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    pub missing: Vec<Missing>,
    pub there: BTreeMap<(String, String), Vec<String>>,
}

/// Of `asks`, the skills not installed for each agent, in folder `cwd`
/// here or its own folder on the machine it runs on, by agent and machine,
/// in their order. An agent the `skills` CLI can't install for is left
/// out, and so is a name that isn't a plain folder name, and a machine
/// that couldn't be asked (logged).
pub fn check(cwd: &Path, asks: &[Ask]) -> Checked {
    let mut out = Checked::default();
    for ask in asks {
        let cli = crate::integrations::by_command(&ask.base);
        let Some(id) = cli.skills_agent() else {
            continue;
        };
        let found = match &ask.there {
            None => {
                let Some(homes) = Homes::here() else {
                    continue;
                };
                let config = crate::chat::commands::config_dir();
                installed(&ask.names, &cli.skill_dirs(cwd, &homes, &config))
            }
            Some((remote, folders)) => {
                let homes = Homes::under(Path::new(&folders.home));
                let dirs =
                    cli.skill_dirs(Path::new(&folders.cwd), &homes, Path::new(&folders.config));
                match installed_there(remote, &ask.names, &dirs) {
                    Ok(found) => {
                        let key = (remote.id.clone(), id.to_owned());
                        out.there.insert(key, found.clone());
                        found
                    }
                    Err(why) => {
                        tracing::warn!("couldn't read the skills on {}: {why}", remote.name);
                        continue;
                    }
                }
            }
        };
        let gone = (ask.names.iter()).filter(|n| plain(n) && !found.contains(n));
        let there = ask.there.as_ref().map(|(remote, _)| remote.clone());
        let at = match (out.missing.iter()).position(|m| m.agent == ask.agent && m.there == there) {
            Some(at) => at,
            None => {
                let (agent, names) = (ask.agent.clone(), Vec::new());
                out.missing.push(Missing {
                    agent,
                    id,
                    names,
                    there,
                });
                out.missing.len() - 1
            }
        };
        for name in gone {
            if !out.missing[at].names.contains(name) {
                out.missing[at].names.push(name.clone());
            }
        }
    }
    out.missing.retain(|m| !m.names.is_empty());
    out
}

/// Those of `names` with a `<name>/SKILL.md` in one of `dirs` on `remote`,
/// asked over its link's master.
fn installed_there(
    remote: &Remote,
    names: &[String],
    dirs: &[PathBuf],
) -> Result<Vec<String>, String> {
    let Some(script) = there_script(names, dirs) else {
        return Ok(Vec::new());
    };
    let reach = super::remote::Reach {
        target: remote.target.clone(),
        control: remote.control.clone(),
    };
    let said = (reach.script(&script, &std::sync::atomic::AtomicBool::new(false)))
        .map_err(|trouble| trouble.text().to_owned())?;
    Ok(found_in(names, &said))
}

/// The `sh` script printing those of `names` with a `<name>/SKILL.md` in
/// one of `dirs`, a line each; none with no plain name to look for.
fn there_script(names: &[String], dirs: &[PathBuf]) -> Option<String> {
    let names: Vec<&str> = (names.iter())
        .filter(|n| plain(n))
        .map(String::as_str)
        .collect();
    if names.is_empty() || dirs.is_empty() {
        return None;
    }
    let dirs: Vec<String> = (dirs.iter())
        .map(|d| crate::desktop::shell_quote(&d.to_string_lossy()))
        .collect();
    Some(format!(
        "for d in {}; do for n in {}; do [ -f \"$d/$n/SKILL.md\" ] && echo \"$n\"; done; done; true\n",
        dirs.join(" "),
        names.join(" ")
    ))
}

/// Those of `names` that `said` (`there_script`'s output) names, in their order.
fn found_in(names: &[String], said: &str) -> Vec<String> {
    let found: Vec<&str> = said.lines().map(str::trim).collect();
    (names.iter())
        .filter(|n| plain(n) && found.contains(&n.as_str()))
        .cloned()
        .collect()
}

/// What the Discover page says of `missing`: "Not installed for Claude
/// Code: grilling, tdd · Codex on desk: tdd".
pub fn missing_words(missing: &[Missing]) -> String {
    let each: Vec<String> = (missing.iter())
        .map(|m| match &m.there {
            None => format!("{}: {}", m.agent, m.names.join(", ")),
            Some(remote) => format!("{} on {}: {}", m.agent, remote.name, m.names.join(", ")),
        })
        .collect();
    format!("Not installed for {}", each.join(" \u{b7} "))
}

/// The lines that install `missing` from `source`, one per machine, this
/// one first: for every agent of it there, for all projects (`-g`), with
/// the `skills` CLI. Another machine's runs over SSH in a login shell
/// there, which finds `npx` as its terminals do.
pub fn install_lines(source: &str, missing: &[Missing]) -> Vec<String> {
    let mut machines: Vec<Option<&Remote>> = Vec::new();
    for m in missing {
        if !machines.contains(&m.there.as_ref()) {
            machines.push(m.there.as_ref());
        }
    }
    machines.sort_by_key(Option::is_some);
    (machines.into_iter())
        .map(|on| {
            let here: Vec<&Missing> = missing.iter().filter(|m| m.there.as_ref() == on).collect();
            let line = install_line(source, &here);
            let quote = crate::desktop::shell_quote;
            match on {
                None => line,
                Some(remote) => format!(
                    "ssh -t -S {} -o ControlMaster=no {} {}",
                    quote(&remote.control.to_string_lossy()),
                    quote(&remote.target),
                    quote(&format!("exec \"$SHELL\" -ilc {}", quote(&line))),
                ),
            }
        })
        .collect()
}

/// The line installing `missing` from `source` for every agent of it.
fn install_line(source: &str, missing: &[&Missing]) -> String {
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
        let ask = |agent: &str, base: &str, of: &[&str]| Ask {
            agent: agent.into(),
            base: base.into(),
            names: names(of),
            there: None,
        };
        let asks = [
            ask("Claude Code", "claude", &["tdd", gone]),
            ask("Claude Code", "claude", &[gone, "../x"]),
            // The skills CLI can't install for it: not told.
            ask("Mine", "my-agent", &[gone]),
        ];
        let checked = check(&cwd, &asks);
        assert_eq!(
            checked.missing,
            [Missing {
                agent: "Claude Code".into(),
                id: "claude-code",
                names: names(&[gone]),
                there: None,
            }]
        );
        assert!(checked.there.is_empty());
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn another_machines_skills_are_found_by_a_script_run_there() {
        let root = std::env::temp_dir().join(format!("gerdr-there-{}", std::process::id()));
        let skill = root.join("b/grilling");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "x").unwrap();
        let dirs = [root.join("a dir"), root.join("b")];
        let wanted = names(&["tdd", "grilling", "../b/grilling"]);
        let script = there_script(&wanted, &dirs).expect("a script");
        let out = (std::process::Command::new("sh").arg("-c").arg(&script))
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert_eq!(found_in(&wanted, &said), names(&["grilling"]));
        assert_eq!(there_script(&names(&["../x"]), &dirs), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_skills_install_in_one_line_per_machine_for_every_agent_without_them() {
        let desk = Remote {
            id: "m1".into(),
            name: "desk".into(),
            target: "ana@desk".into(),
            control: PathBuf::from("/run/c/desk"),
        };
        let gone = |agent: &str, id, of: &[&str], there: Option<&Remote>| Missing {
            agent: agent.into(),
            id,
            names: names(of),
            there: there.cloned(),
        };
        let missing = [
            gone("Pi", "pi", &["tdd"], Some(&desk)),
            gone("Claude Code", "claude-code", &["tdd", "grilling"], None),
            gone("Codex", "codex", &["tdd"], None),
        ];
        assert_eq!(
            install_lines("mattpocock/skills", &missing),
            [
                "npx -y skills@latest add mattpocock/skills -g -s tdd grilling -a claude-code codex -y",
                r#"ssh -t -S /run/c/desk -o ControlMaster=no ana@desk 'exec "$SHELL" -ilc '\''npx -y skills@latest add mattpocock/skills -g -s tdd -a pi -y'\'''"#,
            ]
        );
        assert_eq!(
            missing_words(&missing),
            "Not installed for Pi on desk: tdd \u{b7} Claude Code: tdd, grilling \u{b7} Codex: tdd"
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
